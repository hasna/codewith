import Foundation

/// Typed, high-level calls over the raw `AppServerClient` JSON-RPC transport.
extension AppServerClient {

    // MARK: Threads (sessions)

    /// List sessions with cursor pagination. `cwd` filters to a project.
    func listThreads(cursor: String? = nil, limit: Int = 30)
        async throws -> (threads: [ThreadInfo], nextCursor: String?)
    {
        var params: [String: JSONValue] = [
            "limit": .number(Double(limit)),
            "sortKey": .string("updated_at"),
            "sortDirection": .string("desc"),
        ]
        if let cursor { params["cursor"] = .string(cursor) }
        let r = try await request("thread/list", .object(params), timeout: 30)
        let threads = (r["data"]?.array ?? []).map(ThreadInfo.init(from:))
        let next = r["nextCursor"]?.string
        return (threads, next)
    }

    /// Read a thread's full message history (parsed into chat messages).
    func readThreadMessages(id: String) async throws -> [ChatMessage] {
        let r = try await request("thread/read", .object([
            "threadId": .string(id), "includeTurns": .bool(true),
        ]), timeout: 30)
        return Self.parseThreadMessages(r["thread"] ?? .null)
    }

    func searchThreads(term: String, limit: Int = 40) async throws -> [ThreadInfo] {
        let r = try await request("thread/search", .object([
            "searchTerm": .string(term),
            "limit": .number(Double(limit)),
            "sortKey": .string("updated_at"),
            "sortDirection": .string("desc"),
        ]), timeout: 20)
        return (r["data"]?.array ?? []).map { result in
            ThreadInfo(from: result["thread"] ?? result)
        }
    }

    /// Resume a persisted thread so future `turn/start` requests can continue it.
    func resumeThreadMessages(id: String) async throws -> [ChatMessage] {
        let r = try await request("thread/resume", .object(["threadId": .string(id)]), timeout: 30)
        return Self.parseThreadMessages(r["thread"] ?? .null)
    }

    static func parseThreadMessages(_ thread: JSONValue) -> [ChatMessage] {
        var msgs: [ChatMessage] = []
        for turn in thread["turns"]?.array ?? [] {
            for item in turn["items"]?.array ?? [] {
                if let m = Self.parseItem(item) { msgs.append(m) }
            }
        }
        return msgs
    }

    /// Map a ThreadItem JSON object into a displayable ChatMessage (nil = skip).
    static func parseItem(_ item: JSONValue) -> ChatMessage? {
        switch item["type"]?.string {
        case "userMessage":
            return ChatMessage(role: .user, text: extractText(item["content"]))
        case "agentMessage":
            let t = item["text"]?.string ?? ""
            return t.isEmpty ? nil : ChatMessage(role: .assistant, text: t)
        case "commandExecution":
            let cmd = item["command"]?.string ?? item["command"]?.array?.compactMap { $0.string }.joined(separator: " ") ?? "command"
            return ChatMessage(role: .tool, text: "Ran \(cmd)", toolIcon: "terminal")
        case "fileChange":
            let n = item["changes"]?.array?.count ?? 0
            return ChatMessage(role: .tool, text: n > 0 ? "Edited \(n) file\(n == 1 ? "" : "s")" : "Edited files", toolIcon: "doc.text")
        case "mcpToolCall", "dynamicToolCall":
            let tool = item["tool"]?.string ?? item["server"]?.string ?? "a tool"
            return ChatMessage(role: .tool, text: "Called \(tool)", toolIcon: "wrench.and.screwdriver")
        case "webSearch":
            let q = item["query"]?.string ?? ""
            return ChatMessage(role: .tool, text: q.isEmpty ? "Searched the web" : "Searched: \(q)", toolIcon: "magnifyingglass")
        case "plan":
            let t = item["text"]?.string ?? ""
            return t.isEmpty ? nil : ChatMessage(role: .assistant, text: t)
        default:
            return nil
        }
    }

    /// Extract concatenated text from a `content` array of content items.
    static func extractText(_ content: JSONValue?) -> String {
        guard let content else { return "" }
        if let s = content.string { return s }
        if let arr = content.array {
            return arr.compactMap { $0["text"]?.string }.joined(separator: "\n")
        }
        return ""
    }

    // MARK: Turns (send a message)

    /// Start a new thread, returning its id.
    func startThread(cwd: String?) async throws -> String {
        var params: [String: JSONValue] = [:]
        if let cwd { params["cwd"] = .string(cwd) }
        let r = try await request("thread/start", .object(params), timeout: 30)
        return r["thread"]?["id"]?.string ?? r["threadId"]?.string ?? r["id"]?.string ?? ""
    }

    /// Send a user message to a thread. The server streams the reply via
    /// notifications (handled by the client's `onNotification`).
    func startTurn(threadId: String, input: String, model: String?, provider: String?, effort: String?) async throws -> String? {
        var params: [String: JSONValue] = [
            "threadId": .string(threadId),
            "input": .array([.object(["type": .string("text"), "text": .string(input)])]),
        ]
        if let model { params["model"] = .string(model) }
        if let provider { params["modelProvider"] = .string(provider) }
        if let effort { params["effort"] = .string(effort) }
        let r = try await request("turn/start", .object(params), timeout: 30)
        return r["turn"]?["id"]?.string ?? r["turnId"]?.string
    }

    func interruptTurn(threadId: String, turnId: String) async {
        _ = try? await request("turn/interrupt", .object([
            "threadId": .string(threadId),
            "turnId": .string(turnId),
        ]), timeout: 10)
    }

    // MARK: Loops (schedules + monitors)

    /// Schedules + monitors are per-thread (the protocol has no global list), so
    /// aggregate across the given threads. Field names per the v2 schema.
    func listLoops(threadIds: [String]) async throws -> [LoopInfo] {
        var loops: [LoopInfo] = []
        for tid in threadIds {
            var scheduleCursor: String?
            repeat {
                var params: [String: JSONValue] = ["threadId": .string(tid), "limit": .number(100)]
                if let scheduleCursor { params["cursor"] = .string(scheduleCursor) }
                let sched = try? await request("thread/schedule/list", .object(params), timeout: 15)
                for s in sched?["data"]?.array ?? [] {
                    loops.append(LoopInfo(
                        id: s["scheduleId"]?.string ?? UUID().uuidString,
                        title: s["prompt"]?.string ?? "Schedule",
                        subtitle: Self.scheduleDescription(s["schedule"]),
                        kind: .schedule,
                        active: (s["status"]?.string ?? "active") == "active",
                        threadId: s["threadId"]?.string ?? tid))
                }
                scheduleCursor = sched?["nextCursor"]?.string
            } while scheduleCursor != nil

            var monitorCursor: String?
            repeat {
                var params: [String: JSONValue] = ["threadId": .string(tid), "limit": .number(100)]
                if let monitorCursor { params["cursor"] = .string(monitorCursor) }
                let mon = try? await request("thread/monitor/list", .object(params), timeout: 15)
                for m in mon?["data"]?.array ?? [] {
                    loops.append(LoopInfo(
                        id: m["monitorId"]?.string ?? UUID().uuidString,
                        title: m["name"]?.string ?? m["prompt"]?.string ?? "Monitor",
                        subtitle: m["command"]?.string ?? "monitoring",
                        kind: .monitor,
                        active: (m["status"]?.string ?? "running") == "running",
                        threadId: m["threadId"]?.string ?? tid))
                }
                monitorCursor = mon?["nextCursor"]?.string
            } while monitorCursor != nil
        }
        return loops
    }

    /// Pause/resume a schedule or stop/restart a monitor.
    func setLoopActive(_ loop: LoopInfo, active: Bool) async {
        let method: String
        switch (loop.kind, active) {
        case (.schedule, false): method = "thread/schedule/pause"
        case (.schedule, true):  method = "thread/schedule/resume"
        case (.monitor, false):  method = "thread/monitor/stop"
        case (.monitor, true):   method = "thread/monitor/restart"
        }
        let idKey = loop.kind == .schedule ? "scheduleId" : "monitorId"
        _ = try? await request(method, .object([
            "threadId": .string(loop.threadId), idKey: .string(loop.id),
        ]), timeout: 15)
    }

    static func scheduleDescription(_ s: JSONValue?) -> String {
        guard let s else { return "scheduled" }
        if let expr = s["expression"]?.string { return expr }
        if let amount = s["amount"]?.double, let unit = s["unit"]?.string { return "every \(Int(amount)) \(unit)" }
        if let kind = s["type"]?.string { return kind }
        return "scheduled"
    }

    // MARK: Account & config

    func readAccount() async throws -> AccountInfo {
        let r = try await request("account/read", .object([:]), timeout: 20)
        return AccountInfo(from: r)
    }

    /// Read the current model + provider from config.
    func readModelConfig() async throws -> (model: String?, provider: String?) {
        let r = try await request("config/read", .object([:]), timeout: 20)
        let cfg = r["config"] ?? r
        return (cfg["model"]?.string, cfg["model_provider"]?.string ?? cfg["modelProvider"]?.string)
    }

    /// Read model + provider + approval policy + sandbox mode from config.
    func readFullConfig() async throws -> (model: String?, provider: String?, effort: String?, approval: String?, sandbox: String?) {
        let r = try await request("config/read", .object([:]), timeout: 20)
        let cfg = r["config"] ?? r
        let approval: String?
        if let s = cfg["approval_policy"]?.string { approval = s }
        else if cfg["approval_policy"]?["granular"] != nil { approval = "granular" }
        else { approval = nil }
        return (cfg["model"]?.string,
                cfg["model_provider"]?.string ?? cfg["modelProvider"]?.string,
                cfg["model_reasoning_effort"]?.string ?? cfg["modelReasoningEffort"]?.string,
                approval,
                cfg["sandbox_mode"]?.string)
    }

    /// Write a single config value (config/write, replace strategy).
    func writeConfig(keyPath: String, value: JSONValue) async {
        _ = try? await request("config/value/write", .object([
            "keyPath": .string(keyPath),
            "value": value,
            "mergeStrategy": .string("replace"),
        ]), timeout: 20)
    }

    // MARK: Models / providers / machines

    func listModelProviders() async throws -> [String] {
        let r = try await request("modelProvider/list", .object([:]), timeout: 20)
        return (r["data"]?.array ?? []).compactMap { $0["id"]?.string }
    }

    func listModels(provider: String?) async throws -> [String] {
        var params: [String: JSONValue] = ["limit": .number(200)]
        if let provider { params["modelProvider"] = .string(provider) }
        var out: [String] = []
        var cursor: String?
        var guardCount = 0
        repeat {
            if let cursor { params["cursor"] = .string(cursor) }
            let r = try await request("model/list", .object(params), timeout: 20)
            out.append(contentsOf: (r["data"]?.array ?? []).compactMap {
                $0["model"]?.string ?? $0["id"]?.string
            })
            cursor = r["nextCursor"]?.string
            guardCount += 1
        } while cursor != nil && guardCount < 20
        var seen = Set<String>()
        return out.filter { seen.insert($0).inserted }
    }

    func listMachines() async throws -> [MachineInfo] {
        let r = try await request("machineRegistry/list", .object([
            "includeDisabled": .bool(false),
            "includeForgotten": .bool(false),
            "limit": .number(200),
        ]), timeout: 20)
        return (r["data"]?.array ?? []).map(MachineInfo.init(registryValue:))
    }

    func listApps() async throws -> [AppItemInfo] {
        var out: [AppItemInfo] = []
        var cursor: String?
        var guardCount = 0
        repeat {
            var params: [String: JSONValue] = ["limit": .number(100)]
            if let cursor { params["cursor"] = .string(cursor) }
            guard let r = try? await request("app/list", .object(params), timeout: 20) else { return out }
            out.append(contentsOf: Self.parseApps(r["data"]?.array ?? []))
            cursor = r["nextCursor"]?.string
            guardCount += 1
        } while cursor != nil && guardCount < 20
        return out
    }

    static func parseApps(_ values: [JSONValue]) -> [AppItemInfo] {
        values.map { a in
            AppItemInfo(name: a["name"]?.string ?? a["id"]?.string ?? "App",
                        detail: a["description"]?.string ?? a["summary"]?.string ?? "",
                        enabled: a["isEnabled"]?.bool ?? a["isAccessible"]?.bool ?? true)
        }
    }
}
