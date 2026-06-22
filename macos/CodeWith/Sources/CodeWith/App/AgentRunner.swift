import Foundation

/// Bridges the app to a live agent by shelling out to the installed `codewith`
/// CLI in non-interactive `exec` mode. Returns the agent's textual output, or a
/// classified failure (binary missing / not authenticated / error) so the UI can
/// show a useful message instead of raw logs.
enum AgentRunner {
    enum Outcome {
        case reply(String)
        case notAuthenticated
        case unavailable
        case failed(String)
    }

    /// Candidate locations for the codewith binary (Homebrew on Apple Silicon /
    /// Intel, plus PATH lookup at runtime).
    static let candidatePaths = ["/opt/homebrew/bin/codewith", "/usr/local/bin/codewith"]

    static var binaryPath: String? {
        candidatePaths.first { FileManager.default.isExecutableFile(atPath: $0) }
    }

    static var isAvailable: Bool { binaryPath != nil }

    /// Classify raw CLI output into an outcome (pure function — unit-testable).
    static func classify(exitCode: Int32, output: String) -> Outcome {
        let lower = output.lowercased()
        if lower.contains("401") || lower.contains("unauthorized") || lower.contains("missing bearer") || lower.contains("reconnecting...") {
            return .notAuthenticated
        }
        let trimmed = output.trimmingCharacters(in: .whitespacesAndNewlines)
        if exitCode == 0 && !trimmed.isEmpty { return .reply(trimmed) }
        if trimmed.isEmpty { return .failed("No output from agent.") }
        return .reply(trimmed)
    }

    static func run(prompt: String, cwd: String) async -> Outcome {
        guard let bin = binaryPath else { return .unavailable }
        return await withCheckedContinuation { (cont: CheckedContinuation<Outcome, Never>) in
            let proc = Process()
            proc.executableURL = URL(fileURLWithPath: bin)
            proc.arguments = ["exec", "--skip-git-repo-check", prompt]
            proc.currentDirectoryURL = URL(fileURLWithPath: cwd)
            let pipe = Pipe()
            proc.standardOutput = pipe
            proc.standardError = pipe

            let lock = NSLock()
            var data = Data()
            pipe.fileHandleForReading.readabilityHandler = { h in
                let chunk = h.availableData
                if !chunk.isEmpty { lock.lock(); data.append(chunk); lock.unlock() }
            }
            proc.terminationHandler = { p in
                pipe.fileHandleForReading.readabilityHandler = nil
                lock.lock(); let out = String(data: data, encoding: .utf8) ?? ""; lock.unlock()
                cont.resume(returning: classify(exitCode: p.terminationStatus, output: out))
            }
            do { try proc.run() } catch {
                cont.resume(returning: .failed(error.localizedDescription)); return
            }
            // Hard timeout so the UI never hangs.
            DispatchQueue.global().asyncAfter(deadline: .now() + 90) {
                if proc.isRunning { proc.terminate() }
            }
        }
    }
}
