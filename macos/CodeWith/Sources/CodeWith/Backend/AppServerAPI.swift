import Foundation

/// Typed, high-level calls over the raw `AppServerClient` JSON-RPC transport.
extension AppServerClient {
    enum ThreadAuthProfileUpdate: Equatable {
        case keep
        case set(String)
        case clearDefault
    }

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
        try await readThread(id: id).messages
    }

    func readThread(id: String) async throws -> ThreadReadResult {
        let r = try await request("thread/read", .object([
            "threadId": .string(id), "includeTurns": .bool(true),
        ]), timeout: 30)
        let thread = r["thread"] ?? .null
        return ThreadReadResult(
            messages: Self.parseThreadMessages(thread),
            settings: ThreadSessionSettings(from: thread) ?? ThreadSessionSettings(from: r)
        )
    }

    func searchThreads(term: String, limit: Int = 40) async throws -> [ThreadInfo] {
        var params: [String: JSONValue] = [
            "searchTerm": .string(term),
            "limit": .number(Double(limit)),
            "sortKey": .string("updated_at"),
            "sortDirection": .string("desc"),
        ]
        var out: [ThreadInfo] = []
        var cursor: String?
        var guardCount = 0
        repeat {
            if let cursor { params["cursor"] = .string(cursor) }
            let r = try await request("thread/search", .object(params), timeout: 20)
            out.append(contentsOf: (r["data"]?.array ?? []).map { result in
                ThreadInfo(from: result["thread"] ?? result)
            })
            cursor = r["nextCursor"]?.string
            guardCount += 1
        } while cursor != nil && guardCount < 10
        return out
    }

    /// Resume a persisted thread so future `turn/start` requests can continue it.
    func resumeThreadMessages(id: String) async throws -> [ChatMessage] {
        try await resumeThread(id: id).messages
    }

    func resumeThread(id: String) async throws -> ThreadReadResult {
        let r = try await request("thread/resume", .object(["threadId": .string(id)]), timeout: 30)
        let thread = r["thread"] ?? .null
        return ThreadReadResult(
            messages: Self.parseThreadMessages(thread),
            settings: ThreadSessionSettings(from: thread) ?? ThreadSessionSettings(from: r)
        )
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
    func startTurn(
        threadId: String,
        input: String,
        model: String?,
        provider: String?,
        effort: String?,
        collaborationMode: JSONValue? = nil
    ) async throws -> String? {
        let r = try await request(
            "turn/start",
            Self.turnStartParams(
                threadId: threadId,
                input: input,
                model: model,
                provider: provider,
                effort: effort,
                collaborationMode: collaborationMode),
            timeout: 30)
        return r["turn"]?["id"]?.string ?? r["turnId"]?.string
    }

    static func turnStartParams(
        threadId: String,
        input: String,
        model: String?,
        provider: String?,
        effort: String?,
        collaborationMode: JSONValue? = nil
    ) -> JSONValue {
        var params: [String: JSONValue] = [
            "threadId": .string(threadId),
            "input": .array([.object(["type": .string("text"), "text": .string(input)])]),
        ]
        if let model { params["model"] = .string(model) }
        if let provider { params["modelProvider"] = .string(provider) }
        if let effort { params["effort"] = .string(effort) }
        if let collaborationMode { params["collaborationMode"] = collaborationMode }
        return .object(params)
    }

    static func planCollaborationMode(model: String?, effort: String?) -> JSONValue {
        .object([
            "mode": .string("plan"),
            "settings": .object([
                "model": .string(model ?? "gpt-5.5"),
                "reasoning_effort": effort.map { .string($0) } ?? .null,
                "developer_instructions": .null,
            ]),
        ])
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
        var firstError: Error?
        var successfulEndpointCount = 0
        for tid in threadIds {
            do {
                loops.append(contentsOf: try await listScheduleLoops(threadId: tid))
                successfulEndpointCount += 1
            } catch {
                firstError = firstError ?? error
            }

            do {
                loops.append(contentsOf: try await listMonitorLoops(threadId: tid))
                successfulEndpointCount += 1
            } catch {
                firstError = firstError ?? error
            }
        }
        if successfulEndpointCount == 0, let firstError { throw firstError }
        return loops
    }

    private func listScheduleLoops(threadId: String) async throws -> [LoopInfo] {
        var loops: [LoopInfo] = []
        var cursor: String?
        var seenCursors = Set<String>()
        var pageCount = 0
        repeat {
            if let cursor, !seenCursors.insert(cursor).inserted {
                throw AppServerError.decode("thread/schedule/list repeated cursor \(cursor)")
            }
            var params: [String: JSONValue] = ["threadId": .string(threadId), "limit": .number(100)]
            if let cursor { params["cursor"] = .string(cursor) }
            let sched = try await request("thread/schedule/list", .object(params), timeout: 15)
            loops.append(contentsOf: (sched["data"]?.array ?? []).compactMap {
                guard Self.isLoopSchedule($0) else { return nil }
                return Self.loopInfo(fromSchedule: $0, fallbackThreadId: threadId)
            })
            cursor = sched["nextCursor"]?.string
            pageCount += 1
            if pageCount >= 20, cursor != nil {
                throw AppServerError.decode("thread/schedule/list exceeded pagination limit")
            }
        } while cursor != nil
        return loops
    }

    private func listMonitorLoops(threadId: String) async throws -> [LoopInfo] {
        var loops: [LoopInfo] = []
        var cursor: String?
        var seenCursors = Set<String>()
        var pageCount = 0
        repeat {
            if let cursor, !seenCursors.insert(cursor).inserted {
                throw AppServerError.decode("thread/monitor/list repeated cursor \(cursor)")
            }
            var params: [String: JSONValue] = ["threadId": .string(threadId), "limit": .number(100)]
            if let cursor { params["cursor"] = .string(cursor) }
            let mon = try await request("thread/monitor/list", .object(params), timeout: 15)
            loops.append(contentsOf: (mon["data"]?.array ?? []).map {
                Self.loopInfo(fromMonitor: $0, fallbackThreadId: threadId)
            })
            cursor = mon["nextCursor"]?.string
            pageCount += 1
            if pageCount >= 20, cursor != nil {
                throw AppServerError.decode("thread/monitor/list exceeded pagination limit")
            }
        } while cursor != nil
        return loops
    }

    func createSchedule(
        threadId: String,
        prompt: String,
        promptSource: String = "inline",
        schedule: JSONValue = AppServerClient.dynamicScheduleSpec(),
        timezone: String = TimeZone.current.identifier
    ) async throws -> LoopInfo {
        let r = try await request("thread/schedule/create", Self.threadScheduleCreateParams(
            threadId: threadId,
            prompt: prompt,
            promptSource: promptSource,
            schedule: schedule,
            timezone: timezone
        ), timeout: 20)
        return Self.loopInfo(fromSchedule: r["schedule"] ?? r, fallbackThreadId: threadId)
    }

    func createMonitor(
        threadId: String,
        name: String,
        prompt: String,
        command: String,
        cwd: String? = nil,
        routing: String = "stream",
        outputFile: String? = nil
    ) async throws -> LoopInfo {
        let r = try await request("thread/monitor/create", Self.threadMonitorCreateParams(
            threadId: threadId,
            name: name,
            prompt: prompt,
            command: command,
            cwd: cwd,
            routing: routing,
            outputFile: outputFile
        ), timeout: 20)
        return Self.loopInfo(fromMonitor: r["monitor"] ?? r, fallbackThreadId: threadId)
    }

    /// Pause/resume a schedule or stop/restart a monitor.
    func setLoopActive(_ loop: LoopInfo, active: Bool) async throws {
        let method: String
        switch (loop.kind, active) {
        case (.schedule, false): method = "thread/schedule/pause"
        case (.schedule, true):  method = "thread/schedule/resume"
        case (.monitor, false):  method = "thread/monitor/stop"
        case (.monitor, true):   method = "thread/monitor/restart"
        }
        let idKey = loop.kind == .schedule ? "scheduleId" : "monitorId"
        _ = try await request(method, .object([
            "threadId": .string(loop.threadId), idKey: .string(loop.id),
        ]), timeout: 15)
    }

    func deleteLoop(_ loop: LoopInfo) async throws -> Bool {
        let method = loop.kind == .schedule ? "thread/schedule/delete" : "thread/monitor/delete"
        let idKey = loop.kind == .schedule ? "scheduleId" : "monitorId"
        let r = try await request(method, .object([
            "threadId": .string(loop.threadId),
            idKey: .string(loop.id),
        ]), timeout: 15)
        return r["deleted"]?.bool ?? false
    }

    func runLoopNow(_ loop: LoopInfo) async throws -> Bool {
        guard loop.kind == .schedule else { return false }
        let r = try await request("thread/schedule/runNow", .object([
            "threadId": .string(loop.threadId),
            "scheduleId": .string(loop.id),
        ]), timeout: 15)
        return r["run"] != nil
    }

    static func threadScheduleCreateParams(
        threadId: String,
        prompt: String,
        promptSource: String,
        schedule: JSONValue,
        timezone: String? = nil,
        nextRunAt: Int? = nil,
        expiresAt: Int? = nil
    ) -> JSONValue {
        var params: [String: JSONValue] = [
            "threadId": .string(threadId),
            "prompt": .string(prompt),
            "promptSource": .string(promptSource),
            "schedule": schedule,
        ]
        if let timezone { params["timezone"] = .string(timezone) }
        if let nextRunAt { params["nextRunAt"] = .number(Double(nextRunAt)) }
        if let expiresAt { params["expiresAt"] = .number(Double(expiresAt)) }
        return .object(params)
    }

    static func threadMonitorCreateParams(
        threadId: String,
        name: String,
        prompt: String,
        command: String,
        cwd: String? = nil,
        routing: String? = "stream",
        outputFile: String? = nil
    ) -> JSONValue {
        var params: [String: JSONValue] = [
            "threadId": .string(threadId),
            "name": .string(name),
            "prompt": .string(prompt),
            "command": .string(command),
        ]
        if let cwd { params["cwd"] = .string(cwd) }
        if let routing { params["routing"] = .string(routing) }
        if let outputFile { params["outputFile"] = .string(outputFile) }
        return .object(params)
    }

    static func dynamicScheduleSpec() -> JSONValue {
        .object(["type": .string("dynamic")])
    }

    static func intervalScheduleSpec(amount: Int, unit: LoopScheduleIntervalUnit) -> JSONValue {
        .object([
            "type": .string("interval"),
            "amount": .number(Double(amount)),
            "unit": .string(unit.rawValue),
        ])
    }

    static func cronScheduleSpec(expression: String) -> JSONValue {
        .object([
            "type": .string("cron"),
            "expression": .string(expression),
        ])
    }

    static func isLoopSchedule(_ s: JSONValue) -> Bool {
        s["schedule"]?["type"]?.string != "once"
    }

    static func loopInfo(fromSchedule s: JSONValue, fallbackThreadId: String) -> LoopInfo {
        let status = s["status"]?.string ?? "active"
        return LoopInfo(
            id: s["scheduleId"]?.string ?? UUID().uuidString,
            title: s["prompt"]?.string ?? "Schedule",
            subtitle: Self.scheduleDescription(s["schedule"]),
            kind: .schedule,
            active: status == "active",
            status: status,
            threadId: s["threadId"]?.string ?? fallbackThreadId)
    }

    static func loopInfo(fromMonitor m: JSONValue, fallbackThreadId: String) -> LoopInfo {
        let status = m["status"]?.string ?? "running"
        return LoopInfo(
            id: m["monitorId"]?.string ?? UUID().uuidString,
            title: m["name"]?.string ?? m["prompt"]?.string ?? "Monitor",
            subtitle: m["command"]?.string ?? "monitoring",
            kind: .monitor,
            active: status == "running",
            status: status,
            threadId: m["threadId"]?.string ?? fallbackThreadId)
    }

    static func scheduleDescription(_ s: JSONValue?) -> String {
        guard let s else { return "scheduled" }
        if let expr = s["expression"]?.string { return expr }
        if let amount = s["amount"]?.double, let unit = s["unit"]?.string { return "every \(Int(amount)) \(unit)" }
        if let kind = s["type"]?.string { return kind }
        return "scheduled"
    }

    // MARK: Goals

    func setThreadGoal(threadId: String, objective: String? = nil, status: String? = nil, tokenBudget: Int? = nil)
        async throws -> GoalInfo
    {
        let r = try await request("thread/goal/set", Self.threadGoalSetParams(
            threadId: threadId,
            objective: objective,
            status: status,
            tokenBudget: tokenBudget
        ), timeout: 20)
        return GoalInfo(from: r["goal"] ?? r)
    }

    func setThreadGoal(
        threadId: String,
        objective: String? = nil,
        status: String? = nil,
        tokenBudgetUpdate: GoalTokenBudgetUpdate
    ) async throws -> GoalInfo {
        let r = try await request("thread/goal/set", Self.threadGoalSetParams(
            threadId: threadId,
            objective: objective,
            status: status,
            tokenBudgetUpdate: tokenBudgetUpdate
        ), timeout: 20)
        return GoalInfo(from: r["goal"] ?? r)
    }

    func getThreadGoal(threadId: String) async throws -> GoalInfo? {
        let r = try await request("thread/goal/get", .object(["threadId": .string(threadId)]), timeout: 15)
        guard let goal = r["goal"], !goal.isNull else { return nil }
        return GoalInfo(from: goal)
    }

    func listThreadGoalState(threadId: String, limit: Int = 50) async throws -> ThreadGoalState {
        var goal: GoalInfo?
        var goalPlans: [GoalPlanInfo] = []
        var cursor: String?
        var seenCursors = Set<String>()
        var pageCount = 0
        repeat {
            if let cursor, !seenCursors.insert(cursor).inserted {
                throw AppServerError.decode("thread/goal/list repeated cursor \(cursor)")
            }
            let r = try await request("thread/goal/list", Self.threadGoalListParams(
                threadId: threadId,
                cursor: cursor,
                limit: limit
            ), timeout: 15)
            if goal == nil, let currentGoal = r["goal"], !currentGoal.isNull {
                goal = GoalInfo(from: currentGoal)
            }
            goalPlans.append(contentsOf: (r["goalPlans"]?.array ?? []).map(GoalPlanInfo.init(from:)))
            cursor = r["nextCursor"]?.string
            pageCount += 1
            if pageCount >= 20, cursor != nil {
                throw AppServerError.decode("thread/goal/list exceeded pagination limit")
            }
        } while cursor != nil
        return ThreadGoalState(threadId: threadId, goal: goal, goalPlans: goalPlans)
    }

    func activateGoalPlanNode(threadId: String, nodeId: String) async throws -> ThreadGoalState {
        let r = try await request(
            "thread/goalPlan/activateNode",
            Self.threadGoalPlanActivateNodeParams(threadId: threadId, nodeId: nodeId),
            timeout: 20)
        let goal = r["goal"].map(GoalInfo.init(from:))
        let plan = r["plan"].map(GoalPlanInfo.init(from:))
        return ThreadGoalState(threadId: threadId, goal: goal, goalPlans: plan.map { [$0] } ?? [])
    }

    func listThreadGoals(threadIds: [String]) async throws -> [GoalInfo] {
        var goals: [GoalInfo] = []
        for threadId in threadIds {
            if let state = try? await listThreadGoalState(threadId: threadId),
               let goal = state.goal {
                goals.append(goal)
            }
        }
        return goals
    }

    func listThreadGoalStates(threadIds: [String]) async throws -> [ThreadGoalState] {
        var states: [ThreadGoalState] = []
        var firstError: Error?
        var successCount = 0
        for threadId in threadIds {
            do {
                let state = try await listThreadGoalState(threadId: threadId)
                if state.goal != nil || !state.goalPlans.isEmpty {
                    states.append(state)
                }
                successCount += 1
            } catch {
                firstError = firstError ?? error
            }
        }
        if successCount == 0, let firstError { throw firstError }
        return states
    }

    func clearThreadGoal(threadId: String) async -> Bool {
        let r = try? await request("thread/goal/clear", .object(["threadId": .string(threadId)]), timeout: 15)
        return r?["cleared"]?.bool ?? false
    }

    static func threadGoalSetParams(
        threadId: String,
        objective: String? = nil,
        status: String? = nil,
        tokenBudget: Int? = nil
    ) -> JSONValue {
        var params: [String: JSONValue] = ["threadId": .string(threadId)]
        if let objective { params["objective"] = .string(objective) }
        if let status { params["status"] = .string(status) }
        if let tokenBudget { params["tokenBudget"] = .number(Double(tokenBudget)) }
        return .object(params)
    }

    static func threadGoalSetParams(
        threadId: String,
        objective: String? = nil,
        status: String? = nil,
        tokenBudgetUpdate: GoalTokenBudgetUpdate = .keep
    ) -> JSONValue {
        var params: [String: JSONValue] = ["threadId": .string(threadId)]
        if let objective { params["objective"] = .string(objective) }
        if let status { params["status"] = .string(status) }
        switch tokenBudgetUpdate {
        case .keep:
            break
        case .set(let tokenBudget):
            params["tokenBudget"] = .number(Double(tokenBudget))
        case .clear:
            params["tokenBudget"] = .null
        }
        return .object(params)
    }

    static func threadGoalListParams(threadId: String, cursor: String? = nil, limit: Int = 50) -> JSONValue {
        var params: [String: JSONValue] = [
            "threadId": .string(threadId),
            "limit": .number(Double(limit)),
        ]
        if let cursor { params["cursor"] = .string(cursor) }
        return .object(params)
    }

    static func threadGoalPlanActivateNodeParams(threadId: String, nodeId: String) -> JSONValue {
        .object([
            "threadId": .string(threadId),
            "nodeId": .string(nodeId),
        ])
    }

    // MARK: Workflows

    func listWorkflows(threadIds: [String]) async throws -> [WorkflowInfo] {
        var workflows: [WorkflowInfo] = []
        var firstError: Error?
        var successCount = 0
        for threadId in threadIds {
            do {
                workflows.append(contentsOf: try await listThreadWorkflows(threadId: threadId))
                successCount += 1
            } catch {
                firstError = firstError ?? error
            }

            do {
                workflows.append(contentsOf: try await listThreadWorkflowRuns(threadId: threadId))
                successCount += 1
            } catch {
                firstError = firstError ?? error
            }
        }
        if successCount == 0, let firstError { throw firstError }
        return workflows.sorted { $0.updatedAt > $1.updatedAt }
    }

    private func listThreadWorkflows(threadId: String) async throws -> [WorkflowInfo] {
        var items: [WorkflowInfo] = []
        var cursor: String?
        var seenCursors = Set<String>()
        var pageCount = 0
        repeat {
            if let cursor, !seenCursors.insert(cursor).inserted {
                throw AppServerError.decode("thread/workflow/list repeated cursor \(cursor)")
            }
            var params: [String: JSONValue] = ["threadId": .string(threadId), "limit": .number(100)]
            if let cursor { params["cursor"] = .string(cursor) }
            let r = try await request("thread/workflow/list", .object(params), timeout: 15)
            items.append(contentsOf: (r["data"]?.array ?? []).map {
                WorkflowInfo(workflow: $0, fallbackThreadId: threadId)
            })
            cursor = r["nextCursor"]?.string
            pageCount += 1
            if pageCount >= 20, cursor != nil {
                throw AppServerError.decode("thread/workflow/list exceeded pagination limit")
            }
        } while cursor != nil
        return items
    }

    private func listThreadWorkflowRuns(threadId: String) async throws -> [WorkflowInfo] {
        var items: [WorkflowInfo] = []
        var cursor: String?
        var seenCursors = Set<String>()
        var pageCount = 0
        repeat {
            if let cursor, !seenCursors.insert(cursor).inserted {
                throw AppServerError.decode("thread/workflow/run/list repeated cursor \(cursor)")
            }
            var params: [String: JSONValue] = ["threadId": .string(threadId), "limit": .number(100)]
            if let cursor { params["cursor"] = .string(cursor) }
            let r = try await request("thread/workflow/run/list", .object(params), timeout: 15)
            items.append(contentsOf: (r["data"]?.array ?? []).map {
                WorkflowInfo(run: $0, fallbackThreadId: threadId)
            })
            cursor = r["nextCursor"]?.string
            pageCount += 1
            if pageCount >= 20, cursor != nil {
                throw AppServerError.decode("thread/workflow/run/list exceeded pagination limit")
            }
        } while cursor != nil
        return items
    }

    // MARK: Active sessions

    func listActiveSessions(limit: Int = 50) async throws -> [ActiveSessionPeerInfo] {
        var out: [ActiveSessionPeerInfo] = []
        var cursor: String?
        var guardCount = 0
        repeat {
            var params: [String: JSONValue] = ["limit": .number(Double(limit))]
            if let cursor { params["cursor"] = .string(cursor) }
            let r = try await request("activeSession/list", .object(params), timeout: 15)
            out.append(contentsOf: (r["data"]?.array ?? []).map(ActiveSessionPeerInfo.init(from:)))
            cursor = r["nextCursor"]?.string
            guardCount += 1
        } while cursor != nil && guardCount < 20
        return out
    }

    func sendActiveSessionMessage(
        targetPeerId: String,
        message: String,
        senderThreadId: String?,
        senderLabel: String = "CodeWith.app",
        delivery: String = "triggerTurn"
    ) async throws -> String {
        let r = try await request("activeSession/send", Self.activeSessionSendParams(
            targetPeerId: targetPeerId,
            message: message,
            senderThreadId: senderThreadId,
            senderLabel: senderLabel,
            delivery: delivery
        ), timeout: 20)
        return r["status"]?.string ?? "unsupported"
    }

    static func activeSessionSendParams(
        targetPeerId: String,
        message: String,
        senderThreadId: String? = nil,
        senderLabel: String? = "CodeWith.app",
        delivery: String? = "triggerTurn"
    ) -> JSONValue {
        var params: [String: JSONValue] = [
            "targetPeerId": .string(targetPeerId),
            "message": .string(message),
        ]
        if let senderThreadId { params["senderThreadId"] = .string(senderThreadId) }
        if let senderLabel { params["senderLabel"] = .string(senderLabel) }
        if let delivery { params["delivery"] = .string(delivery) }
        return .object(params)
    }

    // MARK: Durable agents

    func listAgentRuns(limit: Int = 50) async throws -> [AgentRunInfo] {
        var out: [AgentRunInfo] = []
        var cursor: String?
        var guardCount = 0
        repeat {
            let r = try await request("agent/list", Self.agentListParams(cursor: cursor, limit: limit), timeout: 15)
            out.append(contentsOf: (r["data"]?.array ?? []).map(AgentRunInfo.init(from:)))
            cursor = r["nextCursor"]?.string
            guardCount += 1
        } while cursor != nil && guardCount < 20
        return out
    }

    func readAgentRun(agentId: String) async throws -> AgentAttachmentInfo {
        let r = try await request("agent/read", Self.agentIdParams(agentId: agentId), timeout: 15)
        return AgentAttachmentInfo(from: r, fallbackAgentId: agentId)
    }

    func attachAgentRun(agentId: String, cursor: String? = nil, limit: Int = 50) async throws -> AgentAttachmentInfo {
        let r = try await request(
            "agent/attach",
            Self.agentAttachParams(agentId: agentId, cursor: cursor, limit: limit),
            timeout: 20)
        return AgentAttachmentInfo(from: r, fallbackAgentId: agentId)
    }

    func detachAgentRun(agentId: String) async throws -> AgentRunInfo? {
        let r = try await request("agent/detach", Self.agentIdParams(agentId: agentId), timeout: 15)
        guard let agent = r["agent"], !agent.isNull else { return nil }
        return AgentRunInfo(from: agent)
    }

    func stopAgentRun(agentId: String) async throws -> AgentRunInfo? {
        let r = try await request("agent/stop", Self.agentIdParams(agentId: agentId), timeout: 15)
        guard let agent = r["agent"], !agent.isNull else { return nil }
        return AgentRunInfo(from: agent)
    }

    func deleteAgentRun(agentId: String) async throws -> Bool {
        let r = try await request("agent/delete", Self.agentIdParams(agentId: agentId), timeout: 15)
        return r["deleted"]?.bool ?? false
    }

    func listAgentEvents(agentId: String, cursor: String? = nil, limit: Int = 50) async throws -> (data: [JSONValue], nextCursor: String?) {
        let r = try await request(
            "agent/events/list",
            Self.agentEventsListParams(agentId: agentId, cursor: cursor, limit: limit),
            timeout: 15)
        return (r["data"]?.array ?? [], r["nextCursor"]?.string)
    }

    func respondToAgentPendingInteraction(
        agentId: String,
        interactionId: String,
        response: JSONValue,
        terminalStatus: String
    ) async throws -> Bool {
        let r = try await request(
            "agent/pendingInteraction/respond",
            Self.agentPendingInteractionRespondParams(
                agentId: agentId,
                interactionId: interactionId,
                response: response,
                terminalStatus: terminalStatus),
            timeout: 20)
        return r["updated"]?.bool ?? false
    }

    static func agentListParams(cursor: String? = nil, limit: Int = 50) -> JSONValue {
        var params: [String: JSONValue] = ["limit": .number(Double(limit))]
        if let cursor { params["cursor"] = .string(cursor) }
        return .object(params)
    }

    static func agentIdParams(agentId: String) -> JSONValue {
        .object(["agentId": .string(agentId)])
    }

    static func agentAttachParams(agentId: String, cursor: String? = nil, limit: Int = 50) -> JSONValue {
        var params: [String: JSONValue] = [
            "agentId": .string(agentId),
            "limit": .number(Double(limit)),
        ]
        if let cursor { params["cursor"] = .string(cursor) }
        return .object(params)
    }

    static func agentEventsListParams(agentId: String, cursor: String? = nil, limit: Int = 50) -> JSONValue {
        agentAttachParams(agentId: agentId, cursor: cursor, limit: limit)
    }

    static func agentPendingInteractionRespondParams(
        agentId: String,
        interactionId: String,
        response: JSONValue,
        terminalStatus: String
    ) -> JSONValue {
        .object([
            "agentId": .string(agentId),
            "interactionId": .string(interactionId),
            "response": response,
            "terminalStatus": .string(terminalStatus),
        ])
    }

    // MARK: Account & config

    func readAccount() async throws -> AccountInfo {
        let r = try await request("account/read", .object([:]), timeout: 20)
        return AccountInfo(from: r)
    }

    func listAuthProfiles() async throws -> [AuthProfileInfo] {
        do {
            let r = try await request("authProfile/list", .object([:]), timeout: 20)
            return (r["data"]?.array ?? []).map(AuthProfileInfo.init(from:))
        } catch {
            guard Self.isUnsupportedAuthProfileRpc(error) else { throw error }
            return try await ProfileRunner.loadProfiles()
        }
    }

    func switchAuthProfile(_ name: String) async throws -> AuthProfileInfo {
        do {
            let r = try await request("authProfile/switch", .object(["name": .string(name)]), timeout: 20)
            return AuthProfileInfo(from: r["profile"] ?? r)
        } catch {
            guard Self.isUnsupportedAuthProfileRpc(error) else { throw error }
            try await ProfileRunner.switchProfile(name)
            let profiles = try await ProfileRunner.loadProfiles()
            if let switched = profiles.first(where: { $0.name == name && $0.active }) {
                return switched
            }
            throw AppServerError.decode("profile switch did not activate \(name)")
        }
    }

    private static func isUnsupportedAuthProfileRpc(_ error: Swift.Error) -> Bool {
        guard case AppServerError.rpc(let code, let message) = error else { return false }
        return code == -32601 || message.localizedCaseInsensitiveContains("unsupported")
            || message.localizedCaseInsensitiveContains("unknown method")
            || message.localizedCaseInsensitiveContains("not found")
    }

    func listPermissionProfiles(cwd: String? = nil) async throws -> [String] {
        var params: [String: JSONValue] = [:]
        if let cwd, !cwd.isEmpty { params["cwd"] = .string(cwd) }
        let r = try await request("permissionProfile/list", .object(params), timeout: 20)
        return (r["data"]?.array ?? []).compactMap { $0["id"]?.string }
    }

    /// Read the current model + provider from config.
    func readModelConfig() async throws -> (model: String?, provider: String?) {
        let r = try await request("config/read", .object([:]), timeout: 20)
        let cfg = r["config"] ?? r
        return (cfg["model"]?.string, cfg["model_provider"]?.string ?? cfg["modelProvider"]?.string)
    }

    /// Read model + provider + approval policy + sandbox mode from config.
    func readFullConfig() async throws -> (
        model: String?,
        provider: String?,
        effort: String?,
        approval: String?,
        sandbox: String?,
        defaultPermissions: String?,
        developerInstructions: String?,
        desktop: DesktopSettingsInfo
    ) {
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
                cfg["sandbox_mode"]?.string,
                cfg["default_permissions"]?.string ?? cfg["defaultPermissions"]?.string,
                cfg["developer_instructions"]?.string ?? cfg["developerInstructions"]?.string,
                DesktopSettingsInfo(desktop: cfg["desktop"] ?? .null, config: cfg))
    }

    func readConfigRequirements() async throws -> ConfigRequirementsInfo? {
        let r = try await request("configRequirements/read", .null, timeout: 20)
        guard let requirements = r["requirements"], !requirements.isNull else { return nil }
        return ConfigRequirementsInfo(from: requirements)
    }

    /// Write a single config value (config/write, replace strategy).
    @discardableResult
    func writeConfig(keyPath: String, value: JSONValue) async throws -> JSONValue {
        try await request("config/value/write", Self.configWriteParams(keyPath: keyPath, value: value), timeout: 20)
    }

    @discardableResult
    func batchWriteConfig(edits: [(keyPath: String, value: JSONValue)], reloadUserConfig: Bool) async throws -> JSONValue {
        try await request("config/batchWrite", Self.configBatchWriteParams(edits: edits, reloadUserConfig: reloadUserConfig), timeout: 20)
    }

    static func configWriteParams(keyPath: String, value: JSONValue) -> JSONValue {
        .object([
            "keyPath": .string(keyPath),
            "value": value,
            "mergeStrategy": .string("replace"),
        ])
    }

    static func configBatchWriteParams(edits: [(keyPath: String, value: JSONValue)], reloadUserConfig: Bool) -> JSONValue {
        .object([
            "edits": .array(edits.map { edit in
                .object([
                    "keyPath": .string(edit.keyPath),
                    "value": edit.value,
                    "mergeStrategy": .string("replace"),
                ])
            }),
            "reloadUserConfig": .bool(reloadUserConfig),
        ])
    }

    func updateThreadSettings(
        threadId: String,
        model: String? = nil,
        provider: String? = nil,
        effort: String? = nil,
        permissions: String? = nil,
        authProfile: ThreadAuthProfileUpdate = .keep,
        personality: String? = nil
    ) async throws {
        _ = try await request(
            "thread/settings/update",
            Self.threadSettingsUpdateParams(
                threadId: threadId,
                model: model,
                provider: provider,
                effort: effort,
                permissions: permissions,
                authProfile: authProfile,
                personality: personality),
            timeout: 15)
    }

    func updateThreadPersonality(threadId: String, personality: String) async throws {
        _ = try await updateThreadSettings(threadId: threadId, personality: personality)
    }

    func setThreadMemoryMode(threadId: String, enabled: Bool) async throws {
        _ = try await request(
            "thread/memoryMode/set",
            Self.threadMemoryModeSetParams(threadId: threadId, enabled: enabled),
            timeout: 15)
    }

    func resetMemories() async throws {
        _ = try await request("memory/reset", .null, timeout: 30)
    }

    static func threadSettingsUpdateParams(
        threadId: String,
        model: String? = nil,
        provider: String? = nil,
        effort: String? = nil,
        permissions: String? = nil,
        authProfile: ThreadAuthProfileUpdate = .keep,
        personality: String? = nil
    ) -> JSONValue {
        var params: [String: JSONValue] = ["threadId": .string(threadId)]
        if let model { params["model"] = .string(model) }
        if let provider { params["modelProvider"] = .string(provider) }
        if let effort { params["effort"] = .string(effort) }
        if let permissions { params["permissions"] = .string(permissions) }
        switch authProfile {
        case .keep:
            break
        case .set(let name):
            params["authProfile"] = .string(name)
        case .clearDefault:
            params["authProfile"] = .null
        }
        if let personality { params["personality"] = .string(personality) }
        return .object(params)
    }

    static func threadSettingsUpdatePersonalityParams(threadId: String, personality: String) -> JSONValue {
        threadSettingsUpdateParams(threadId: threadId, personality: personality)
    }

    static func threadMemoryModeSetParams(threadId: String, enabled: Bool) -> JSONValue {
        .object([
            "threadId": .string(threadId),
            "mode": .string(enabled ? "enabled" : "disabled"),
        ])
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
        var out: [MachineInfo] = []
        var cursor: String?
        var guardCount = 0
        repeat {
            var params: [String: JSONValue] = [
                "includeDisabled": .bool(false),
                "includeForgotten": .bool(false),
                "limit": .number(200),
            ]
            if let cursor { params["cursor"] = .string(cursor) }
            let r = try await request("machineRegistry/list", .object(params), timeout: 20)
            out.append(contentsOf: (r["data"]?.array ?? []).map(MachineInfo.init(registryValue:)))
            cursor = r["nextCursor"]?.string
            guardCount += 1
        } while cursor != nil && guardCount < 20
        return out
    }

    func startMachinePairing(manualCode: Bool = true) async throws -> MachinePairingInfo {
        let r = try await request(
            "remoteControl/pairing/start",
            Self.remoteControlPairingStartParams(manualCode: manualCode),
            timeout: 20)
        return MachinePairingInfo(from: r)
    }

    func machinePairingClaimed(_ pairing: MachinePairingInfo) async throws -> Bool {
        let r = try await request(
            "remoteControl/pairing/status",
            Self.remoteControlPairingStatusParams(pairing: pairing),
            timeout: 20)
        return r["claimed"]?.bool ?? false
    }

    static func remoteControlPairingStartParams(manualCode: Bool) -> JSONValue {
        .object(["manualCode": .bool(manualCode)])
    }

    static func remoteControlPairingStatusParams(pairing: MachinePairingInfo) -> JSONValue {
        if let manualPairingCode = pairing.manualPairingCode, !manualPairingCode.isEmpty {
            return .object(["manualPairingCode": .string(manualPairingCode)])
        }
        return .object(["pairingCode": .string(pairing.pairingCode)])
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
