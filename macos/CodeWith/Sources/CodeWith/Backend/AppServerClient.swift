import Foundation
#if canImport(Darwin)
import Darwin
#elseif canImport(Glibc)
import Glibc
#endif

enum AppServerError: Error, LocalizedError {
    case binaryNotFound
    case notRunning
    case rpc(code: Int, message: String)
    case decode(String)
    case timeout(String)

    var errorDescription: String? {
        switch self {
        case .binaryNotFound: return "The codewith CLI was not found. Install it to enable live data."
        case .notRunning: return "The app-server is not running."
        case .rpc(let code, let message): return "app-server error \(code): \(message)"
        case .decode(let m): return "Failed to decode app-server response: \(m)"
        case .timeout(let m): return "Timed out waiting for \(m)."
        }
    }
}

/// JSON-RPC client driving `codewith app-server` over stdio (newline-delimited
/// JSON). Hardened: all stdin writes serialized on a write queue; all parsing
/// on a serial parse queue (preserving notification order); continuations
/// resolved immediately on write/dead-process failure; child process killed on
/// teardown; bounded buffer; re-entrant start (rebuilds Process/Pipe).
final class AppServerClient: @unchecked Sendable {
    struct ServerRequest: Sendable, Equatable {
        var id: JSONValue
        var method: String
        var params: JSONValue
    }

    static var candidateBinaries: [String] {
        var paths: [String] = []
        let env = ProcessInfo.processInfo.environment
        if let override = env["CODEWITH_CLI_PATH"], !override.isEmpty {
            paths.append(override)
        }
        if let resource = Bundle.main.url(forResource: "codewith", withExtension: nil) {
            paths.append(resource.path)
        }
        paths.append(Bundle.main.bundleURL.appendingPathComponent("Contents/MacOS/codewith").path)
        paths.append(contentsOf: [
            "/opt/homebrew/bin/codewith", "/usr/local/bin/codewith",
            "\(NSHomeDirectory())/.bun/bin/codewith",
        ])
        paths.append(contentsOf: (env["PATH"] ?? "")
            .split(separator: ":")
            .map { URL(fileURLWithPath: String($0)).appendingPathComponent("codewith").path })
        var seen = Set<String>()
        return paths.filter { seen.insert($0).inserted }
    }
    static var binaryPath: String? {
        candidateBinaries.first {
            FileManager.default.isExecutableFile(atPath: $0) && !isSelfExecutable($0)
        }
    }

    /// Guard against spawning the app's own GUI binary as the "CLI". macOS uses a
    /// case-insensitive filesystem, so the candidate `Contents/MacOS/codewith`
    /// resolves to the app executable `Contents/MacOS/CodeWith`; launching it
    /// reboots the GUI, which spawns again — an unbounded fork bomb. Compare by
    /// file identity (device + inode), which sees through the case folding.
    static func isSelfExecutable(_ path: String) -> Bool {
        guard let selfPath = Bundle.main.executablePath else { return false }
        return sameFile(path, selfPath)
    }

    /// True iff both paths resolve to the same on-disk file (same device + inode).
    static func sameFile(_ a: String, _ b: String) -> Bool {
        var sa = stat(), sb = stat()
        guard stat(a, &sa) == 0, stat(b, &sb) == 0 else { return false }
        return sa.st_dev == sb.st_dev && sa.st_ino == sb.st_ino
    }
    var isAvailable: Bool { Self.binaryPath != nil }

    private let lock = NSLock()
    private let writeQueue = DispatchQueue(label: "codewith.appserver.write")
    private let parseQueue = DispatchQueue(label: "codewith.appserver.parse")
    private let decoder = JSONDecoder()
    private let maxBuffer = 32 * 1024 * 1024

    // Rebuilt on each start() so reconnect works.
    private var process: Process?
    private var stdinHandle: FileHandle?
    private var stdoutHandle: FileHandle?
    private var processGeneration = 0

    private var nextId = 0
    private var pending: [Int: (Result<JSONValue, Error>) -> Void] = [:]
    private var buffer = Data()   // only touched on parseQueue
    private var running = false

    /// Ordered stream of server notifications `(method, params)`.
    let notifications: AsyncStream<(String, JSONValue)>
    private let notifyContinuation: AsyncStream<(String, JSONValue)>.Continuation
    /// Ordered stream of app-server requests that require a client response.
    let serverRequests: AsyncStream<ServerRequest>
    private let serverRequestContinuation: AsyncStream<ServerRequest>.Continuation
    var onExit: (@Sendable (Int32) -> Void)?

    init() {
        var c: AsyncStream<(String, JSONValue)>.Continuation!
        notifications = AsyncStream(bufferingPolicy: .unbounded) { c = $0 }
        notifyContinuation = c
        var r: AsyncStream<ServerRequest>.Continuation!
        serverRequests = AsyncStream(bufferingPolicy: .unbounded) { r = $0 }
        serverRequestContinuation = r
    }

    deinit { stop() }

    // MARK: Lifecycle

    func start(binary: String? = nil) throws {
        guard let bin = binary ?? Self.binaryPath else { throw AppServerError.binaryNotFound }
        lock.lock(); let already = running; lock.unlock()
        if already { return }

        let proc = Process()
        let inPipe = Pipe()
        let outPipe = Pipe()
        proc.executableURL = URL(fileURLWithPath: bin)
        proc.arguments = ["app-server"]
        proc.standardInput = inPipe
        proc.standardOutput = outPipe
        proc.standardError = FileHandle.nullDevice
        var env = ProcessInfo.processInfo.environment
        env["PATH"] = (env["PATH"] ?? "") + ":/opt/homebrew/bin:/usr/local/bin"
        proc.environment = env

        lock.lock()
        processGeneration += 1
        let generation = processGeneration
        lock.unlock()

        outPipe.fileHandleForReading.readabilityHandler = { [weak self] h in
            guard let self else { return }
            let data = h.availableData
            if data.isEmpty { self.parseQueue.async { self.handleEOF(generation: generation) } }
            else { self.parseQueue.async { self.ingest(data, generation: generation) } }
        }
        proc.terminationHandler = { [weak self] p in
            guard let self else { return }
            if self.markStopped(generation: generation) {
                self.failAllPending(AppServerError.notRunning)
                self.onExit?(p.terminationStatus)
            }
        }

        try proc.run()
        let started = proc.isRunning
        lock.lock()
        if started {
            process = proc
            stdinHandle = inPipe.fileHandleForWriting
            stdoutHandle = outPipe.fileHandleForReading
            running = true
        } else {
            process = nil
            stdinHandle = nil
            stdoutHandle = nil
            running = false
        }
        buffer.removeAll(keepingCapacity: false)
        lock.unlock()
        if !started {
            outPipe.fileHandleForReading.readabilityHandler = nil
            try? inPipe.fileHandleForWriting.close()
        }
    }

    func stop() {
        lock.lock()
        let proc = process
        let sin = stdinHandle
        let sout = stdoutHandle
        processGeneration += 1
        running = false
        process = nil
        stdinHandle = nil
        stdoutHandle = nil
        lock.unlock()
        sout?.readabilityHandler = nil
        try? sin?.close()
        if proc?.isRunning == true { proc?.terminate() }
        failAllPending(AppServerError.notRunning)
    }

    private func markStopped(generation: Int) -> Bool {
        lock.lock()
        guard generation == processGeneration else {
            lock.unlock()
            return false
        }
        running = false
        process = nil
        stdinHandle = nil
        stdoutHandle?.readabilityHandler = nil
        stdoutHandle = nil
        lock.unlock()
        return true
    }

    private func handleEOF(generation: Int) {
        lock.lock()
        let shouldStop = running && generation == processGeneration
        lock.unlock()
        if shouldStop {
            if markStopped(generation: generation) {
                failAllPending(AppServerError.notRunning)
            }
        }
    }

    // MARK: Ingest (parseQueue only)

    private func ingest(_ data: Data, generation: Int? = nil) {
        if let generation, !isCurrentGeneration(generation) { return }
        buffer.append(data)
        if buffer.count > maxBuffer {
            buffer.removeAll(keepingCapacity: false)
            failAllPending(AppServerError.decode("buffer overflow"))
            return
        }
        while let nl = buffer.firstIndex(of: 0x0A) {
            if let generation, !isCurrentGeneration(generation) {
                buffer.removeAll(keepingCapacity: false)
                return
            }
            let line = buffer.subdata(in: buffer.startIndex..<nl)
            buffer.removeSubrange(buffer.startIndex...nl)
            if !line.isEmpty { route(line) }
        }
    }

    private func isCurrentGeneration(_ generation: Int) -> Bool {
        lock.lock()
        let current = running && generation == processGeneration
        lock.unlock()
        return current
    }

    /// Pure classification of one JSON-RPC frame (no lock, no dispatch) — testable.
    enum Incoming: Equatable {
        case response(id: Int, result: JSONValue)
        case failure(id: Int, code: Int, message: String)
        case notification(method: String, params: JSONValue)
        case serverRequest(id: JSONValue, method: String, params: JSONValue)
        case ignored
    }
    static func classify(_ line: Data) -> Incoming {
        guard let msg = try? JSONDecoder().decode(JSONValue.self, from: line) else { return .ignored }
        if case .number(let idn)? = msg["id"], msg["result"] != nil || msg["error"] != nil {
            let id = Int(idn)
            if let err = msg["error"], !err.isNull {
                return .failure(id: id, code: err["code"]?.int ?? -1,
                                message: err["message"]?.string ?? "unknown")
            }
            return .response(id: id, result: msg["result"] ?? .null)
        }
        if let id = msg["id"], let method = msg["method"]?.string {
            return .serverRequest(id: id, method: method, params: msg["params"] ?? .null)
        }
        if let method = msg["method"]?.string, msg["id"] == nil {
            return .notification(method: method, params: msg["params"] ?? .null)
        }
        return .ignored
    }

    private func route(_ line: Data) {
        switch Self.classify(line) {
        case .response(let id, let result):
            lock.lock(); let h = pending.removeValue(forKey: id); lock.unlock()
            h?(.success(result))
        case .failure(let id, let code, let message):
            lock.lock(); let h = pending.removeValue(forKey: id); lock.unlock()
            h?(.failure(AppServerError.rpc(code: code, message: message)))
        case .notification(let method, let params):
            notifyContinuation.yield((method, params))
        case .serverRequest(let id, let method, let params):
            serverRequestContinuation.yield(ServerRequest(id: id, method: method, params: params))
        case .ignored:
            break
        }
    }

    #if DEBUG
    /// Test hook: feed raw bytes as if they arrived from the server.
    func _ingestForTesting(_ data: Data) { ingest(data) }
    #endif

    private func failAllPending(_ error: Error) {
        lock.lock(); let handlers = Array(pending.values); pending.removeAll(); lock.unlock()
        handlers.forEach { $0(.failure(error)) }
    }

    private func failPending(_ id: Int, _ error: Error) {
        lock.lock(); let h = pending.removeValue(forKey: id); lock.unlock()
        h?(.failure(error))
    }

    // MARK: Requests

    func notify(_ method: String, _ params: JSONValue = .null) {
        enqueueWrite(.object(["method": .string(method), "params": params]))
    }

    func request(_ method: String, _ params: JSONValue = .null, timeout: TimeInterval = 30) async throws -> JSONValue {
        let id = lock.withLock {
            nextId += 1
            return nextId
        }
        let env = JSONValue.object(["id": .number(Double(id)), "method": .string(method), "params": params])
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { (cont: CheckedContinuation<JSONValue, Error>) in
                lock.lock(); pending[id] = { cont.resume(with: $0) }; lock.unlock()
                writeQueue.async { [weak self] in
                    guard let self else { return }
                    if !self.writeSync(env) { self.failPending(id, AppServerError.notRunning) }
                }
                DispatchQueue.global().asyncAfter(deadline: .now() + timeout) { [weak self] in
                    self?.failPending(id, AppServerError.timeout(method))
                }
            }
        } onCancel: {
            self.failPending(id, CancellationError())
        }
    }

    func respond(to id: JSONValue, result: JSONValue) {
        enqueueWrite(.object(["id": id, "result": result]))
    }

    func respondError(to id: JSONValue, code: Int = -32603, message: String) {
        enqueueWrite(.object([
            "id": id,
            "error": .object([
                "code": .number(Double(code)),
                "message": .string(message),
            ]),
        ]))
    }

    private func enqueueWrite(_ value: JSONValue) {
        writeQueue.async { [weak self] in _ = self?.writeSync(value) }
    }

    /// Serialized stdin write (writeQueue only). Returns false on dead pipe.
    private func writeSync(_ value: JSONValue) -> Bool {
        guard var data = try? JSONEncoder().encode(value) else { return false }
        data.append(0x0A)
        lock.lock(); let handle = stdinHandle; let r = running; lock.unlock()
        guard r, let handle else { return false }
        do { try handle.write(contentsOf: data); return true }
        catch { return false }
    }

    // MARK: Handshake

    @discardableResult
    func initialize(clientName: String = "CodeWith.app", version: String = "1.0") async throws -> JSONValue {
        let result = try await request("initialize", .object([
            "clientInfo": .object([
                "name": .string(clientName), "title": .string("CodeWith"), "version": .string(version),
            ]),
            "capabilities": .object([
                "experimentalApi": .bool(true),
            ]),
        ]))
        notify("initialized")
        return result
    }
}
