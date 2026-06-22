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
