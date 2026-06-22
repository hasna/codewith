import Foundation

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
    static let candidateBinaries = [
        "/opt/homebrew/bin/codewith", "/usr/local/bin/codewith",
        "\(NSHomeDirectory())/.bun/bin/codewith",
    ]
    static var binaryPath: String? {
        candidateBinaries.first { FileManager.default.isExecutableFile(atPath: $0) }
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

    private var nextId = 0
    private var pending: [Int: (Result<JSONValue, Error>) -> Void] = [:]
    private var buffer = Data()   // only touched on parseQueue
    private var running = false

    /// Ordered stream of server notifications `(method, params)`.
    let notifications: AsyncStream<(String, JSONValue)>
    private let notifyContinuation: AsyncStream<(String, JSONValue)>.Continuation
    var onExit: (@Sendable (Int32) -> Void)?

    init() {
        var c: AsyncStream<(String, JSONValue)>.Continuation!
        notifications = AsyncStream(bufferingPolicy: .unbounded) { c = $0 }
        notifyContinuation = c
    }

    deinit { stop() }

    // MARK: Lifecycle

    func start() throws {
        guard let bin = Self.binaryPath else { throw AppServerError.binaryNotFound }
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

        outPipe.fileHandleForReading.readabilityHandler = { [weak self] h in
            guard let self else { return }
            let data = h.availableData
            if data.isEmpty { self.parseQueue.async { self.handleEOF() } }
            else { self.parseQueue.async { self.ingest(data) } }
        }
        proc.terminationHandler = { [weak self] p in
            guard let self else { return }
            self.markStopped()
            self.failAllPending(AppServerError.notRunning)
            self.onExit?(p.terminationStatus)
        }

        try proc.run()
        lock.lock()
        process = proc
        stdinHandle = inPipe.fileHandleForWriting
        running = true
        buffer.removeAll(keepingCapacity: false)
        lock.unlock()
    }

    func stop() {
        lock.lock()
        let proc = process
        let sin = stdinHandle
        running = false
        process = nil
        stdinHandle = nil
        lock.unlock()
        try? sin?.close()
        if proc?.isRunning == true { proc?.terminate() }
        failAllPending(AppServerError.notRunning)
    }

    private func markStopped() {
        lock.lock(); running = false; process = nil; stdinHandle = nil; lock.unlock()
    }

    private func handleEOF() {
        lock.lock(); let r = running; lock.unlock()
        if r { markStopped(); failAllPending(AppServerError.notRunning) }
    }

    // MARK: Ingest (parseQueue only)

    private func ingest(_ data: Data) {
        buffer.append(data)
        if buffer.count > maxBuffer {
            buffer.removeAll(keepingCapacity: false)
            failAllPending(AppServerError.decode("buffer overflow"))
            return
        }
        while let nl = buffer.firstIndex(of: 0x0A) {
            let line = buffer.subdata(in: buffer.startIndex..<nl)
            buffer.removeSubrange(buffer.startIndex...nl)
            if !line.isEmpty { route(line) }
        }
    }

    /// Pure classification of one JSON-RPC frame (no lock, no dispatch) — testable.
    enum Incoming: Equatable {
        case response(id: Int, result: JSONValue)
        case failure(id: Int, code: Int, message: String)
        case notification(method: String, params: JSONValue)
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
        lock.lock(); nextId += 1; let id = nextId; lock.unlock()
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
            "capabilities": .object([:]),
        ]))
        notify("initialized")
        return result
    }
}
