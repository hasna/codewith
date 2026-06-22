import Foundation

/// Reads the real machine fleet by shelling out to the `machines` CLI
/// (`machines topology -j`). Returns parsed machines + the local machine id.
enum FleetRunner {
    static let candidates = [
        "\(NSHomeDirectory())/.bun/bin/machines", "/opt/homebrew/bin/machines", "/usr/local/bin/machines",
    ]
    static var binaryPath: String? { candidates.first { FileManager.default.isExecutableFile(atPath: $0) } }

    static func loadFleet() async -> (machines: [MachineInfo], localId: String) {
        guard let bin = binaryPath else { return ([], "") }
        let output: String = await withCheckedContinuation { cont in
            let proc = Process()
            proc.executableURL = URL(fileURLWithPath: bin)
            proc.arguments = ["topology", "-j"]
            var env = ProcessInfo.processInfo.environment
            env["PATH"] = (env["PATH"] ?? "") + ":/opt/homebrew/bin:/usr/local/bin:\(NSHomeDirectory())/.bun/bin"
            proc.environment = env
            let pipe = Pipe(); proc.standardOutput = pipe; proc.standardError = FileHandle.nullDevice
            proc.terminationHandler = { _ in
                let data = pipe.fileHandleForReading.readDataToEndOfFile()
                cont.resume(returning: String(data: data, encoding: .utf8) ?? "")
            }
            do { try proc.run() } catch { cont.resume(returning: "") }
            DispatchQueue.global().asyncAfter(deadline: .now() + 20) { if proc.isRunning { proc.terminate() } }
        }
        return parse(output)
    }

    static func parse(_ json: String) -> (machines: [MachineInfo], localId: String) {
        guard let data = json.data(using: .utf8),
              let root = try? JSONDecoder().decode(JSONValue.self, from: data) else { return ([], "") }
        let localId = root["local_machine_id"]?.string ?? ""
        let machines = (root["machines"]?.array ?? []).map { m -> MachineInfo in
            let id = m["machine_id"]?.string ?? m["hostname"]?.string ?? "machine"
            return MachineInfo(
                id: id,
                os: m["os"]?.string ?? m["platform"]?.string ?? "unknown",
                status: m["heartbeat_status"]?.string ?? "unknown",
                role: (m["tags"]?.array?.compactMap { $0.string }.first) ?? (m["user"]?.string ?? ""),
                isLocal: id == localId)
        }
        return (machines, localId)
    }
}

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
            // Split on runs of 2+ spaces.
            let cols = line.components(separatedBy: "  ").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
            guard cols.count >= 2 else { continue }
            let name = cols[0]
            let email = cols[1]
            let providerMode = cols.count > 2 ? cols[2] : ""
            let provider = providerMode.split(separator: " ").first.map(String.init) ?? providerMode
            let plan = cols.count > 3 ? cols[cols.count - 1] : ""
            out.append(AuthProfileInfo(name: name, email: email, provider: provider, plan: plan))
        }
        return out
    }
}
