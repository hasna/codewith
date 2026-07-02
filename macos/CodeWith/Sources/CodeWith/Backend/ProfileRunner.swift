import Foundation

/// Reads auth profiles via `codewith profile list` (fixed-width text table).
enum ProfileRunner {
    static func loadProfiles() async -> [AuthProfileInfo] {
        guard let bin = AppServerClient.binaryPath else { return [] }
        let output: String = await withCheckedContinuation { cont in
            let proc = Process()
            proc.executableURL = URL(fileURLWithPath: bin)
            proc.arguments = ["profile", "list"]
            var env = ProcessInfo.processInfo.environment
            env["PATH"] = (env["PATH"] ?? "") + ":/opt/homebrew/bin:/usr/local/bin:\(NSHomeDirectory())/.bun/bin"
            proc.environment = env
            let pipe = Pipe(); proc.standardOutput = pipe; proc.standardError = FileHandle.nullDevice
            proc.terminationHandler = { _ in
                let d = pipe.fileHandleForReading.readDataToEndOfFile()
                cont.resume(returning: String(data: d, encoding: .utf8) ?? "")
            }
            do { try proc.run() } catch { cont.resume(returning: "") }
            DispatchQueue.global().asyncAfter(deadline: .now() + 15) { if proc.isRunning { proc.terminate() } }
        }
        return parse(output)
    }

    /// Switch the active profile via `codewith profile switch <name>`.
    static func switchProfile(_ name: String) async {
        guard let bin = AppServerClient.binaryPath else { return }
        _ = await withCheckedContinuation { (cont: CheckedContinuation<Void, Never>) in
            let proc = Process()
            proc.executableURL = URL(fileURLWithPath: bin)
            proc.arguments = ["profile", "switch", name]
            var env = ProcessInfo.processInfo.environment
            env["PATH"] = (env["PATH"] ?? "") + ":/opt/homebrew/bin:/usr/local/bin:\(NSHomeDirectory())/.bun/bin"
            proc.environment = env
            proc.standardOutput = FileHandle.nullDevice; proc.standardError = FileHandle.nullDevice
            proc.terminationHandler = { _ in cont.resume() }
            do { try proc.run() } catch { cont.resume() }
            DispatchQueue.global().asyncAfter(deadline: .now() + 15) { if proc.isRunning { proc.terminate() } }
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
