import Foundation

/// A session (thread) summary as returned by `thread/list` / `thread/read`.
struct ThreadInfo: Identifiable, Hashable {
    let id: String
    var name: String
    var cwd: String?
    var preview: String?
    var updatedAt: String?
    var createdAt: String?
    var modelProvider: String?
    var status: String?
    var machineId: String?
    var gitOriginUrl: String?
    var gitBranch: String?
    var gitSha: String?
    var messages: [ChatMessage] = []

    init(from v: JSONValue) {
        id = v["id"]?.string ?? UUID().uuidString
        let nm = v["name"]?.string
        let pv = v["preview"]?.string
        name = (nm?.isEmpty == false ? nm : nil) ?? (pv?.isEmpty == false ? pv : nil) ?? "Untitled session"
        cwd = v["cwd"]?.string
        preview = pv
        updatedAt = v["updatedAt"]?.string ?? v["updatedAt"]?.double.map { String(Int($0)) }
        createdAt = v["createdAt"]?.string ?? v["createdAt"]?.double.map { String(Int($0)) }
        modelProvider = v["modelProvider"]?.string
        // status is an object {type: "idle"|"active"|...} on the wire.
        status = v["status"]?["type"]?.string ?? v["status"]?.string
        machineId = v["machineId"]?.string
            ?? v["sourceMachineId"]?.string
            ?? v["targetMachineId"]?.string
            ?? v["machine"]?["machineId"]?.string
            ?? v["machine"]?["id"]?.string
        gitOriginUrl = v["gitInfo"]?["originUrl"]?.string
        gitBranch = v["gitInfo"]?["branch"]?.string
        gitSha = v["gitInfo"]?["sha"]?.string
    }

    /// Repo-identity grouping key: normalized git origin if present, else cwd.
    var projectKey: String? {
        if let o = gitOriginUrl { return ProjectInfo.normalizeOrigin(o) }
        if let c = cwd, !c.isEmpty { return c }
        return nil
    }

    /// Short relative-age label (e.g. "3w", "1m") parsed best-effort from updatedAt.
    var ageLabel: String {
        guard let updatedAt, let ts = ThreadInfo.parseDate(updatedAt) else { return "" }
        let secs = Date().timeIntervalSince(ts)
        switch secs {
        case ..<60: return "now"
        case ..<3600: return "\(Int(secs/60))m"
        case ..<86400: return "\(Int(secs/3600))h"
        case ..<604800: return "\(Int(secs/86400))d"
        case ..<2_592_000: return "\(Int(secs/604800))w"
        default: return "\(Int(secs/2_592_000))mo"
        }
    }

    static func parseDate(_ s: String) -> Date? {
        if let d = Double(s) { return Date(timeIntervalSince1970: d > 1_000_000_000_000 ? d/1000 : d) }
        let iso = ISO8601DateFormatter()
        iso.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return iso.date(from: s) ?? ISO8601DateFormatter().date(from: s)
    }
}

/// Thread-scoped settings returned by thread read/resume/update RPCs.
struct ThreadSessionSettings: Hashable {
    var model: String?
    var provider: String?
    var effort: String?
    var permissionProfileId: String?
    var authProfile: String?

    init(
        model: String? = nil,
        provider: String? = nil,
        effort: String? = nil,
        permissionProfileId: String? = nil,
        authProfile: String? = nil
    ) {
        self.model = model
        self.provider = provider
        self.effort = effort
        self.permissionProfileId = permissionProfileId
        self.authProfile = authProfile
    }

    init?(from v: JSONValue) {
        let settings = v["threadSettings"] ?? v["settings"] ?? v["thread"]?["threadSettings"] ?? v["thread"]?["settings"] ?? v
        self.init(
            model: settings["model"]?.string,
            provider: settings["modelProvider"]?.string ?? settings["model_provider"]?.string,
            effort: settings["effort"]?.string,
            permissionProfileId: settings["activePermissionProfile"]?["id"]?.string
                ?? settings["active_permission_profile"]?["id"]?.string
                ?? settings["permissions"]?.string,
            authProfile: settings["authProfile"]?.string ?? settings["auth_profile"]?.string
        )
        if model == nil, provider == nil, effort == nil, permissionProfileId == nil, authProfile == nil {
            return nil
        }
    }
}

struct ThreadReadResult: Hashable {
    var messages: [ChatMessage]
    var settings: ThreadSessionSettings?
}

/// A project = a repo / working directory that has sessions. Grouped by git
/// origin when available (so sub-dirs of one repo are one project), else cwd.
struct ProjectInfo: Identifiable, Hashable {
    var id: String { groupKey }
    var name: String
    var path: String
    var groupKey: String
    var originUrl: String?
    var branch: String?
    var threadCount: Int
    var lastActivity: Int

    /// Derive projects from threads, grouping by repo identity (origin) or cwd.
    static func derive(from threads: [ThreadInfo]) -> [ProjectInfo] {
        struct Acc { var name: String; var path: String; var origin: String?; var branch: String?; var count: Int; var last: Int }
        var acc: [String: Acc] = [:]
        var order: [String] = []
        for t in threads {
            guard let cwd = t.cwd, !cwd.isEmpty else { continue }
            let key = t.projectKey ?? cwd
            let updated = Int(t.updatedAt ?? "") ?? 0
            if var a = acc[key] {
                a.count += 1
                if updated >= a.last { a.last = updated; a.branch = t.gitBranch ?? a.branch }
                acc[key] = a
            } else {
                order.append(key)
                acc[key] = Acc(
                    name: t.gitOriginUrl.map(repoName(fromOrigin:)) ?? (cwd as NSString).lastPathComponent,
                    path: cwd, origin: t.gitOriginUrl, branch: t.gitBranch, count: 1, last: updated)
            }
        }
        return order.map { k in
            let a = acc[k]!
            return ProjectInfo(name: a.name, path: a.path, groupKey: k, originUrl: a.origin,
                               branch: a.branch, threadCount: a.count, lastActivity: a.last)
        }
    }

    static func repoName(fromOrigin url: String) -> String {
        var s = url.hasSuffix(".git") ? String(url.dropLast(4)) : url
        if let slash = s.lastIndex(of: "/") { s = String(s[s.index(after: slash)...]) }
        else if let colon = s.lastIndex(of: ":") { s = String(s[s.index(after: colon)...]) }
        return s.isEmpty ? url : s
    }
    static func normalizeOrigin(_ url: String) -> String {
        var s = url.hasSuffix(".git") ? String(url.dropLast(4)) : url
        s = s.replacingOccurrences(of: "git@github.com:", with: "github.com/")
             .replacingOccurrences(of: "https://github.com/", with: "github.com/")
             .replacingOccurrences(of: "ssh://git@github.com/", with: "github.com/")
        return s.lowercased()
    }
}

/// A machine in the fleet from the app-server machine registry.
struct MachineInfo: Identifiable, Hashable {
    var id: String { machineId }
    var machineId: String
    var displayName: String
    var os: String
    var status: String   // online / offline / unknown
    var role: String
    var isLocal: Bool
    var online: Bool { status == "online" }

    init(id: String, os: String, status: String, role: String, isLocal: Bool) {
        self.machineId = id
        self.displayName = id
        self.os = os
        self.status = status
        self.role = role
        self.isLocal = isLocal
    }

    init(registryValue v: JSONValue) {
        machineId = v["machineId"]?.string ?? v["displayName"]?.string ?? UUID().uuidString
        displayName = v["displayName"]?.string ?? machineId
        os = v["capabilities"]?["os"]?.string
            ?? v["capabilities"]?["platform"]?.string
            ?? v["adapterName"]?.string
            ?? "unknown"
        status = (v["healthState"]?.string ?? "unknown").lowercased()
        let source = v["sourceKind"]?.string ?? ""
        let trust = v["trustState"]?.string ?? ""
        role = [source, trust].filter { !$0.isEmpty }.joined(separator: " · ")
        isLocal = (v["trustState"]?.string ?? "").lowercased() == "local"
            || (v["sourceKind"]?.string ?? "").lowercased() == "local"
    }
}

struct MachinePairingInfo: Hashable {
    var pairingCode: String
    var manualPairingCode: String?
    var environmentId: String
    var expiresAt: Int

    init(from v: JSONValue) {
        pairingCode = v["pairingCode"]?.string ?? ""
        manualPairingCode = v["manualPairingCode"]?.string
        environmentId = v["environmentId"]?.string ?? ""
        expiresAt = v["expiresAt"]?.int ?? 0
    }

    var displayCode: String {
        if let manualPairingCode, !manualPairingCode.isEmpty {
            return manualPairingCode
        }
        return pairingCode
    }
}

struct DesktopSettingsInfo: Hashable {
    var workMode: String
    var fileOpenDestination: String
    var language: String
    var showMenuBar: Bool
    var bottomPanel: Bool
    var personality: String
    var memoryEnabled: Bool
    var chronicleResearch: Bool
    var skipToolAssistedChats: Bool

    init(
        workMode: String = "coding",
        fileOpenDestination: String = "cursor",
        language: String = "auto",
        showMenuBar: Bool = true,
        bottomPanel: Bool = true,
        personality: String = "pragmatic",
        memoryEnabled: Bool = true,
        chronicleResearch: Bool = false,
        skipToolAssistedChats: Bool = false
    ) {
        self.workMode = workMode
        self.fileOpenDestination = fileOpenDestination
        self.language = language
        self.showMenuBar = showMenuBar
        self.bottomPanel = bottomPanel
        self.personality = personality
        self.memoryEnabled = memoryEnabled
        self.chronicleResearch = chronicleResearch
        self.skipToolAssistedChats = skipToolAssistedChats
    }

    init(from v: JSONValue) {
        self.init(
            workMode: v["workMode"]?.string ?? "coding",
            fileOpenDestination: v["fileOpenDestination"]?.string ?? "cursor",
            language: v["language"]?.string ?? "auto",
            showMenuBar: v["showMenuBar"]?.bool ?? true,
            bottomPanel: v["bottomPanel"]?.bool ?? true,
            personality: v["personality"]?.string ?? "pragmatic",
            memoryEnabled: v["memoryEnabled"]?.bool ?? true,
            chronicleResearch: v["chronicleResearch"]?.bool ?? false,
            skipToolAssistedChats: v["skipToolAssistedChats"]?.bool ?? false
        )
    }

    init(desktop: JSONValue, config: JSONValue) {
        self.init(from: desktop)
        if desktop["fileOpenDestination"]?.string == nil {
            switch config["file_opener"]?.string {
            case "cursor":
                fileOpenDestination = "cursor"
            case "none":
                fileOpenDestination = "system"
            default:
                break
            }
        }
        personality = config["personality"]?.string ?? personality

        let features = config["features"] ?? .null
        let memories = config["memories"] ?? .null
        let memoriesFeatureEnabled = features["memories"]?.bool ?? false
        let useMemories = memories["use_memories"]?.bool ?? true
        let generateMemories = memories["generate_memories"]?.bool ?? true
        memoryEnabled = memoriesFeatureEnabled && useMemories && generateMemories
        chronicleResearch = features["chronicle"]?.bool ?? false
        skipToolAssistedChats = memories["disable_on_external_context"]?.bool
            ?? memories["no_memories_if_mcp_or_web_search"]?.bool
            ?? false
    }
}

struct ConfigRequirementsInfo: Hashable {
    var allowedApprovalPolicies: [String]?
    var allowedSandboxModes: [String]?

    init(from v: JSONValue) {
        allowedApprovalPolicies = Self.approvalPolicyArray(v["allowedApprovalPolicies"])
        allowedSandboxModes = Self.stringArray(v["allowedSandboxModes"])
    }

    static func approvalPolicyArray(_ value: JSONValue?) -> [String]? {
        guard let array = value?.array else { return nil }
        return array.compactMap { item in
            if let string = item.string { return string }
            if item["granular"] != nil { return "granular" }
            return nil
        }
    }

    static func stringArray(_ value: JSONValue?) -> [String]? {
        guard let array = value?.array else { return nil }
        return array.compactMap(\.string)
    }

    func approvalOptions(defaults: [String]) -> [String] {
        allowedApprovalPolicies?.filter { defaults.contains($0) } ?? defaults
    }

    func sandboxOptions(defaults: [String]) -> [String] {
        allowedSandboxModes?.filter { defaults.contains($0) } ?? defaults
    }

    func allowsSandbox(_ mode: String) -> Bool {
        allowedSandboxModes?.contains(mode) ?? true
    }
}

/// An auth profile from `authProfile/list`.
struct AuthProfileInfo: Identifiable, Hashable {
    var id: String { name }
    var name: String
    var email: String
    var provider: String
    var plan: String
    var active: Bool = false

    init(name: String, email: String, provider: String, plan: String, active: Bool = false) {
        self.name = name
        self.email = email
        self.provider = provider
        self.plan = plan
        self.active = active
    }

    init(from v: JSONValue) {
        name = v["name"]?.string ?? "profile"
        email = v["email"]?.string ?? v["accountId"]?.string ?? ""
        provider = v["subscriptionProvider"]?.string ?? v["provider"]?.string ?? ""
        plan = v["plan"]?.string ?? v["authMode"]?.string ?? ""
        active = v["active"]?.bool ?? false
    }
}

/// An installable app/skill from `app/list`.
struct AppItemInfo: Identifiable, Hashable {
    var id: String { name }
    var name: String
    var detail: String
    var enabled: Bool
}

/// A loop = a schedule or monitor running against a thread.
struct LoopInfo: Identifiable, Hashable {
    let id: String
    var title: String
    var subtitle: String
    var kind: Kind
    var active: Bool
    var status: String = ""
    var threadId: String = ""
    enum Kind: String { case schedule, monitor }

    var canToggle: Bool {
        switch kind {
        case .schedule:
            return status.isEmpty || status == "active" || status == "paused"
        case .monitor:
            return status.isEmpty || status == "running" || status == "stopped" || status == "failed"
        }
    }

    var canRunNow: Bool {
        kind == .schedule && (status.isEmpty || status == "active" || status == "paused")
    }

    var toggleLabel: String {
        switch (kind, active) {
        case (.schedule, true): return "Pause"
        case (.schedule, false): return "Resume"
        case (.monitor, true): return "Stop"
        case (.monitor, false): return "Restart"
        }
    }
}

enum LoopScheduleIntervalUnit: String, Hashable {
    case minutes, hours, days
}

enum LoopCreationKind: String, CaseIterable, Hashable {
    case schedule, monitor
}

enum LoopCreationScheduleMode: String, CaseIterable, Hashable {
    case dynamic, interval, cron
}

enum LoopMonitorRouting: String, CaseIterable, Hashable {
    case stream, file, both

    var writesToFile: Bool {
        self == .file || self == .both
    }
}

struct LoopCreationDraft: Hashable {
    static let defaultPrompt = "Continue this thread and report anything that needs attention."

    var kind: LoopCreationKind = .schedule
    var prompt: String = defaultPrompt
    var scheduleMode: LoopCreationScheduleMode = .dynamic
    var intervalAmount: String = "5"
    var intervalUnit: LoopScheduleIntervalUnit = .minutes
    var cronExpression: String = ""
    var monitorName: String = ""
    var command: String = ""
    var cwd: String = ""
    var routing: LoopMonitorRouting = .stream
    var outputFile: String = ""

    var normalizedPrompt: String {
        let trimmed = prompt.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? Self.defaultPrompt : trimmed
    }

    var normalizedMonitorName: String {
        let trimmed = monitorName.trimmingCharacters(in: .whitespacesAndNewlines)
        if !trimmed.isEmpty { return trimmed }
        let firstLine = normalizedPrompt
            .split(whereSeparator: \.isNewline)
            .first
            .map(String.init) ?? "Monitor"
        return firstLine.isEmpty ? "Monitor" : String(firstLine.prefix(80))
    }

    var intervalAmountValue: Int? {
        guard let value = Int(intervalAmount.trimmingCharacters(in: .whitespacesAndNewlines)),
              value > 0
        else {
            return nil
        }
        return value
    }

    var normalizedCronExpression: String {
        cronExpression.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    var normalizedCommand: String {
        command.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    var normalizedCwd: String? {
        let trimmed = cwd.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }

    var normalizedOutputFile: String? {
        let trimmed = outputFile.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }

    var validationMessage: String? {
        switch kind {
        case .schedule:
            switch scheduleMode {
            case .dynamic:
                return nil
            case .interval:
                return intervalAmountValue == nil ? "Enter a positive interval." : nil
            case .cron:
                return normalizedCronExpression.isEmpty ? "Enter a cron expression." : nil
            }
        case .monitor:
            if normalizedCommand.isEmpty { return "Enter a monitor command." }
            if routing.writesToFile && normalizedOutputFile == nil {
                return "Enter an output file for file routing."
            }
            if !routing.writesToFile && normalizedOutputFile != nil {
                return "Output files are only valid for file routing."
            }
            return nil
        }
    }

    var canCreate: Bool {
        validationMessage == nil
    }
}

enum GoalTokenBudgetUpdate: Equatable {
    case keep
    case set(Int)
    case clear
}

/// A persisted goal attached to a thread.
struct GoalInfo: Identifiable, Hashable {
    var id: String
    var threadId: String
    var objective: String
    var status: String
    var tokenBudget: Int?
    var tokensUsed: Int
    var timeUsedSeconds: Int

    init(from v: JSONValue) {
        id = v["goalId"]?.string ?? v["id"]?.string ?? UUID().uuidString
        threadId = v["threadId"]?.string ?? ""
        objective = v["objective"]?.string ?? ""
        status = v["status"]?.string ?? "active"
        tokenBudget = v["tokenBudget"]?.int
        tokensUsed = v["tokensUsed"]?.int ?? 0
        timeUsedSeconds = v["timeUsedSeconds"]?.int ?? 0
    }
}

/// A durable goal plan attached to a thread.
struct GoalPlanInfo: Identifiable, Hashable {
    var id: String { planId }
    var planId: String
    var threadId: String
    var status: String
    var autoExecute: String
    var nodeCount: Int
    var completedNodeCount: Int
    var activeNodeCount: Int
    var pendingNodeCount: Int
    var nodes: [GoalPlanNodeInfo]

    init(from v: JSONValue) {
        planId = v["planId"]?.string ?? v["id"]?.string ?? UUID().uuidString
        threadId = v["threadId"]?.string ?? ""
        status = v["status"]?.string ?? "active"
        autoExecute = v["autoExecute"]?.string ?? "off"
        nodeCount = v["nodeCount"]?.int ?? 0
        completedNodeCount = v["completedNodeCount"]?.int ?? 0
        activeNodeCount = v["activeNodeCount"]?.int ?? 0
        pendingNodeCount = v["pendingNodeCount"]?.int ?? 0
        nodes = (v["nodes"]?.array ?? []).map(GoalPlanNodeInfo.init(from:))
    }

    var progressText: String {
        "\(completedNodeCount)/\(max(nodeCount, nodes.count)) complete"
    }
}

struct GoalPlanNodeInfo: Identifiable, Hashable {
    var id: String { nodeId }
    var nodeId: String
    var planId: String
    var threadId: String
    var key: String
    var objective: String
    var status: String
    var ready: Bool

    init(from v: JSONValue) {
        nodeId = v["nodeId"]?.string ?? v["id"]?.string ?? UUID().uuidString
        planId = v["planId"]?.string ?? ""
        threadId = v["threadId"]?.string ?? ""
        key = v["key"]?.string ?? ""
        objective = v["objective"]?.string ?? ""
        status = v["status"]?.string ?? "pending"
        ready = v["ready"]?.bool ?? false
    }

    var canActivate: Bool {
        ready && status == "pending"
    }
}

struct ThreadGoalState: Identifiable, Hashable {
    var id: String { threadId }
    var threadId: String
    var goal: GoalInfo?
    var goalPlans: [GoalPlanInfo]
}

/// A workflow spec or run attached to a thread.
struct WorkflowInfo: Identifiable, Hashable {
    enum Kind: String, Hashable {
        case workflow, run
    }

    var id: String
    var threadId: String
    var title: String
    var subtitle: String
    var status: String
    var kind: Kind
    var updatedAt: Int

    init(workflow v: JSONValue, fallbackThreadId: String) {
        let recordId = v["workflowRecordId"]?.string ?? v["id"]?.string ?? UUID().uuidString
        id = "workflow:\(recordId)"
        threadId = v["threadId"]?.string ?? fallbackThreadId
        title = v["displayName"]?.string ?? v["specWorkflowId"]?.string ?? "Workflow"
        status = v["status"]?.string ?? "draft"
        kind = .workflow
        updatedAt = v["updatedAt"]?.int ?? 0
        let steps = v["stepCount"]?.int ?? 0
        let agents = v["agentCount"]?.int ?? 0
        subtitle = "\(steps) step\(steps == 1 ? "" : "s") · \(agents) agent\(agents == 1 ? "" : "s")"
    }

    init(run v: JSONValue, fallbackThreadId: String) {
        let runId = v["runId"]?.string ?? v["id"]?.string ?? UUID().uuidString
        id = "run:\(runId)"
        threadId = v["threadId"]?.string ?? fallbackThreadId
        title = "Run \(runId.prefix(8))"
        status = v["status"]?.string ?? "pending"
        kind = .run
        updatedAt = v["updatedAt"]?.int ?? 0
        let succeeded = v["succeededStepCount"]?.int ?? 0
        let failed = v["failedStepCount"]?.int ?? 0
        let active = v["activeStepCount"]?.int ?? 0
        subtitle = "\(succeeded) succeeded · \(failed) failed · \(active) active"
    }
}

/// A currently loaded app-server peer that can receive active-session messages.
struct ActiveSessionPeerInfo: Identifiable, Hashable {
    var id: String { peerId }
    var peerId: String
    var threadId: String
    var displayName: String
    var kind: String
    var cwd: String
    var capabilities: [String]
    var canTriggerTurn: Bool { capabilities.contains("triggerTurn") }
    var canQueueMessage: Bool { capabilities.contains("queueMessage") }
    var canReceiveMessage: Bool { capabilities.contains("receiveMessage") || canQueueMessage }

    init(from v: JSONValue) {
        peerId = v["peerId"]?.string ?? v["threadId"]?.string ?? UUID().uuidString
        threadId = v["threadId"]?.string ?? ""
        kind = v["kind"]?.string ?? "codewithSession"
        cwd = v["cwd"]?.string ?? ""
        let label = v["displayName"]?.string
            ?? v["agentPath"]?.string
            ?? (cwd.isEmpty ? nil : (cwd as NSString).lastPathComponent)
        displayName = label?.isEmpty == false ? label! : peerId
        capabilities = v["capabilities"]?.array?.compactMap(\.string) ?? []
    }

    var menuSubtitle: String {
        switch kind {
        case "spawnedAgent": return cwd.isEmpty ? "Active spawned agent" : "Agent in \(cwd)"
        case "bridgeAdapter": return "Active bridge peer"
        default: return cwd.isEmpty ? "Active CodeWith session" : "Session in \(cwd)"
        }
    }
}

/// A durable background-agent run from `agent/list`.
struct AgentRunInfo: Identifiable, Hashable {
    var id: String { agentId }
    var agentId: String
    var threadId: String
    var status: String
    var desiredState: String
    var retentionState: String
    var source: String
    var rolloutPath: String

    init(from v: JSONValue) {
        agentId = v["agentId"]?.string ?? v["id"]?.string ?? UUID().uuidString
        threadId = v["threadId"]?.string ?? ""
        status = v["status"]?.string ?? "queued"
        desiredState = v["desiredState"]?.string ?? "running"
        retentionState = v["retentionState"]?.string ?? "active"
        source = v["source"]?.string ?? "agent"
        rolloutPath = v["rolloutPath"]?.string ?? ""
    }

    var canOpenThread: Bool { !threadId.isEmpty }
    var isDeleted: Bool { desiredState == "deleted" || retentionState == "deleted" }
    var displayName: String { "Agent \(agentId.prefix(8))" }

    var menuSubtitle: String {
        var parts = [statusDisplay]
        if !rolloutPath.isEmpty { parts.append((rolloutPath as NSString).lastPathComponent) }
        return parts.joined(separator: " · ")
    }

    private var statusDisplay: String {
        status
            .replacingOccurrences(of: "waitingOn", with: "waiting on ")
            .replacingOccurrences(of: "([a-z])([A-Z])", with: "$1 $2", options: .regularExpression)
            .lowercased()
    }
}

struct AgentPendingInteractionInfo: Identifiable {
    var id: String { interactionId }
    var interactionId: String
    var agentId: String
    var kind: String
    var status: String
    var requestPayload: JSONValue

    init(from v: JSONValue) {
        interactionId = v["interactionId"]?.string ?? v["id"]?.string ?? UUID().uuidString
        agentId = v["agentId"]?.string ?? ""
        kind = v["kind"]?.string ?? "interaction"
        status = v["status"]?.string ?? "pending"
        requestPayload = v["requestPayload"] ?? .null
    }

    var summary: String {
        let title = requestPayload["title"]?.string
            ?? requestPayload["message"]?.string
            ?? requestPayload["prompt"]?.string
            ?? kind
        return title.isEmpty ? kind : title
    }
}

struct AgentAttachmentInfo {
    var agent: AgentRunInfo?
    var status: String
    var summary: String
    var eventCount: Int
    var pendingInteractions: [AgentPendingInteractionInfo]

    init(from v: JSONValue, fallbackAgentId: String) {
        agent = v["agent"].flatMap { value in
            value.isNull ? nil : AgentRunInfo(from: value)
        }
        let statusSnapshot = v["statusSnapshot"] ?? .null
        status = statusSnapshot["status"]?.string
            ?? agent?.status
            ?? "unknown"
        summary = statusSnapshot["summary"]?.string ?? ""
        eventCount = v["events"]?.array?.count ?? v["data"]?.array?.count ?? 0
        pendingInteractions = (v["pendingInteractions"]?.array ?? [])
            .map(AgentPendingInteractionInfo.init(from:))
        if agent == nil {
            agent = AgentRunInfo(from: .object(["agentId": .string(fallbackAgentId), "status": .string(status)]))
        }
    }

    var pendingCount: Int {
        pendingInteractions.count
    }
}

/// Account / profile info from `account/read`.
struct AccountInfo {
    var name: String
    var email: String
    var plan: String
    var initials: String
    var requiresOpenAIAuth: Bool

    init(from v: JSONValue) {
        // account/read → { account: Account|null, requiresOpenaiAuth }
        requiresOpenAIAuth = v["requiresOpenaiAuth"]?.bool ?? true
        let acc = v["account"] ?? .null
        if acc.isNull {
            if requiresOpenAIAuth {
                name = "Signed out"; email = ""; plan = ""; initials = "?"
            } else {
                name = "Local provider"; email = ""; plan = "No account required"; initials = "LP"
            }
            return
        }
        email = acc["email"]?.string ?? ""
        plan = acc["planType"]?.string ?? acc["plan"]?.string ?? ""
        name = acc["displayName"]?.string ?? acc["name"]?.string
            ?? (email.isEmpty ? (acc["type"]?.string ?? "Account") : email)
        let parts = name.split(separator: " ")
        let derived = parts.prefix(2).compactMap { $0.first }.map(String.init).joined().uppercased()
        initials = derived.isEmpty ? "?" : derived
    }
    static let signedOut = AccountInfo(from: .object(["name": .string("Signed out")]))
}
