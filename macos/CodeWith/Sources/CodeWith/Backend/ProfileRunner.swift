import Foundation

private final class ProfileCommandContinuation: @unchecked Sendable {
    private let lock = NSLock()
    private var didResume = false
    private let continuation: CheckedContinuation<String, Swift.Error>

    init(_ continuation: CheckedContinuation<String, Swift.Error>) {
        self.continuation = continuation
    }

    func resume(with result: Result<String, Swift.Error>) {
        lock.lock()
        guard !didResume else {
            lock.unlock()
            return
        }
        didResume = true
        lock.unlock()
        continuation.resume(with: result)
    }
}

/// Reads auth profiles via `codewith profile list` (fixed-width text table).
enum ProfileRunner {
    enum Error: LocalizedError {
        case binaryNotFound
        case launchFailed(String)
        case commandFailed(arguments: [String], status: Int32, stderr: String)
        case timedOut(arguments: [String])

        var errorDescription: String? {
            switch self {
            case .binaryNotFound:
                return "The codewith CLI was not found."
            case .launchFailed(let message):
                return "Could not run codewith profile command: \(message)"
            case .commandFailed(let arguments, let status, let stderr):
                let command = (["codewith"] + arguments).joined(separator: " ")
                let detail = stderr.trimmingCharacters(in: .whitespacesAndNewlines)
                return detail.isEmpty
                    ? "\(command) exited with status \(status)."
                    : "\(command) exited with status \(status): \(detail)"
            case .timedOut(let arguments):
                return "\((["codewith"] + arguments).joined(separator: " ")) timed out."
            }
        }
    }

    static func loadProfiles() async throws -> [AuthProfileInfo] {
        parse(try await run(arguments: ["profile", "list"], captureOutput: true))
    }

    /// Switch the active profile via `codewith profile switch <name>`.
    static func switchProfile(_ name: String) async throws {
        _ = try await run(arguments: ["profile", "switch", name], captureOutput: false)
    }

    private static func run(arguments: [String], captureOutput: Bool) async throws -> String {
        guard let bin = AppServerClient.binaryPath else { throw Error.binaryNotFound }
        return try await withCheckedThrowingContinuation { cont in
            let completion = ProfileCommandContinuation(cont)
            let proc = Process()
            proc.executableURL = URL(fileURLWithPath: bin)
            proc.arguments = arguments
            var env = ProcessInfo.processInfo.environment
            env["PATH"] = (env["PATH"] ?? "") + ":/opt/homebrew/bin:/usr/local/bin:\(NSHomeDirectory())/.bun/bin"
            proc.environment = env
            let outputPipe = Pipe()
            let errorPipe = Pipe()
            proc.standardOutput = captureOutput ? outputPipe : FileHandle.nullDevice
            proc.standardError = errorPipe

            proc.terminationHandler = { process in
                let output = captureOutput ? outputPipe.fileHandleForReading.readDataToEndOfFile() : Data()
                let errorData = errorPipe.fileHandleForReading.readDataToEndOfFile()
                let stderr = String(data: errorData, encoding: .utf8) ?? ""
                if process.terminationStatus == 0 {
                    completion.resume(with: .success(String(data: output, encoding: .utf8) ?? ""))
                } else {
                    completion.resume(with: .failure(Error.commandFailed(
                        arguments: arguments,
                        status: process.terminationStatus,
                        stderr: stderr)))
                }
            }
            do {
                try proc.run()
            } catch {
                completion.resume(with: .failure(Error.launchFailed(error.localizedDescription)))
            }
            DispatchQueue.global().asyncAfter(deadline: .now() + 15) {
                if proc.isRunning {
                    proc.terminate()
                    completion.resume(with: .failure(Error.timedOut(arguments: arguments)))
                }
            }
        }
    }

    static func parse(_ text: String) -> [AuthProfileInfo] {
        var out: [AuthProfileInfo] = []
        for raw in text.split(separator: "\n") {
            let line = raw.trimmingCharacters(in: .whitespaces)
            if line.isEmpty || line.hasPrefix("NAME") { continue }
            let active = line.hasPrefix("*")
            let cleaned = active ? String(line.dropFirst()).trimmingCharacters(in: .whitespaces) : line
            // Split on runs of 2+ spaces.
            let cols = cleaned.components(separatedBy: "  ").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
            guard cols.count >= 2 else { continue }
            let name = cols[0]
            let email = cols[1]
            let providerMode = cols.count > 2 ? cols[2] : ""
            let provider = providerMode.split(separator: " ").first.map(String.init) ?? providerMode
            let plan = cols.count > 3 ? cols[cols.count - 1] : ""
            out.append(AuthProfileInfo(name: name, email: email, provider: provider, plan: plan, active: active))
        }
        return out
    }
}
