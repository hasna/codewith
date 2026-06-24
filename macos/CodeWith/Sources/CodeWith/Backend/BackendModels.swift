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
        allowedApprovalPolicies = Self.stringArray(v["allowedApprovalPolicies"])
        allowedSandboxModes = Self.stringArray(v["allowedSandboxModes"])
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
        provider = Self.providerDisplayName(v["subscriptionProvider"]?.string ?? v["provider"]?.string ?? "")
        plan = v["plan"]?.string ?? v["authMode"]?.string ?? ""
        active = v["active"]?.bool ?? false
    }

    private static func providerDisplayName(_ provider: String) -> String {
        switch provider {
        case "chatgpt": return "ChatGPT"
        case "claudeAi": return "Claude.ai"
        case "cursor": return "Cursor"
        case "grok": return "Grok"
        default: return provider
        }
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

    var canCreate: Bool {
        if normalizedPrompt.isEmpty { return false }
        switch kind {
        case .schedule:
            switch scheduleMode {
            case .dynamic:
                return true
            case .interval:
                return intervalAmountValue != nil
            case .cron:
                return !normalizedCronExpression.isEmpty
            }
        case .monitor:
            return !normalizedCommand.isEmpty && (!routing.writesToFile || normalizedOutputFile != nil)
        }
    }

    var validationMessage: String {
        if normalizedPrompt.isEmpty { return "Enter a prompt." }
        switch kind {
        case .schedule:
            if scheduleMode == .interval && intervalAmountValue == nil {
                return "Enter a positive interval."
            }
            if scheduleMode == .cron && normalizedCronExpression.isEmpty {
                return "Enter a cron expression."
            }
        case .monitor:
            if normalizedCommand.isEmpty { return "Enter a command to monitor." }
            if routing.writesToFile && normalizedOutputFile == nil {
                return "Choose an output file for file routing."
            }
        }
        return ""
    }
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

struct AccountUsageInfo: Hashable {
    var lifetimeTokens: Int?
    var peakDailyTokens: Int?
    var longestRunningTurnSec: Int?
    var currentStreakDays: Int?
    var longestStreakDays: Int?
    var dailyBuckets: [AccountUsageBucket]

    init(from v: JSONValue) {
        let summary = v["summary"] ?? .null
        lifetimeTokens = summary["lifetimeTokens"]?.int
        peakDailyTokens = summary["peakDailyTokens"]?.int
        longestRunningTurnSec = summary["longestRunningTurnSec"]?.int
        currentStreakDays = summary["currentStreakDays"]?.int
        longestStreakDays = summary["longestStreakDays"]?.int
        dailyBuckets = (v["dailyUsageBuckets"]?.array ?? []).map(AccountUsageBucket.init(from:))
    }
}

struct AccountUsageBucket: Identifiable, Hashable {
    var id: String { startDate }
    var startDate: String
    var tokens: Int

    init(from v: JSONValue) {
        startDate = v["startDate"]?.string ?? ""
        tokens = v["tokens"]?.int ?? 0
    }
}

struct McpServerStatusInfo: Identifiable, Hashable {
    var id: String { name }
    var name: String
    var authStatus: String
    var toolCount: Int
    var resourceCount: Int

    init(from v: JSONValue) {
        name = v["name"]?.string ?? "server"
        authStatus = v["authStatus"]?["type"]?.string ?? v["authStatus"]?.string ?? "unknown"
        toolCount = v["tools"]?.object?.count ?? 0
        resourceCount = (v["resources"]?.array ?? []).count
            + (v["resourceTemplates"]?.array ?? []).count
    }
}

struct HookEntryInfo: Identifiable, Hashable {
    var id: String { cwd }
    var cwd: String
    var hooks: [HookInfo]
    var warnings: [String]
    var errors: [String]

    init(from v: JSONValue) {
        cwd = v["cwd"]?.string ?? ""
        hooks = (v["hooks"]?.array ?? []).map(HookInfo.init(from:))
        warnings = (v["warnings"]?.array ?? []).compactMap(\.string)
        errors = (v["errors"]?.array ?? []).compactMap { error in
            error["message"]?.string
        }
    }
}

struct HookInfo: Identifiable, Hashable {
    var id: String { key }
    var key: String
    var eventName: String
    var handlerType: String
    var matcher: String?
    var command: String?
    var enabled: Bool
    var trustStatus: String

    init(from v: JSONValue) {
        key = v["key"]?.string ?? UUID().uuidString
        eventName = v["eventName"]?.string ?? ""
        handlerType = v["handlerType"]?.string ?? ""
        matcher = v["matcher"]?.string
        command = v["command"]?.string
        enabled = v["enabled"]?.bool ?? false
        trustStatus = v["trustStatus"]?.string ?? "unknown"
    }
}

struct WorktreeInfo: Identifiable, Hashable {
    var id: String { worktreeId }
    var worktreeId: String
    var baseRepoPath: String
    var worktreePath: String
    var branch: String?
    var lifecycleStatus: String
    var dirty: Bool
    var ownerKind: String
    var updatedAt: Int

    init(from v: JSONValue) {
        worktreeId = v["worktreeId"]?.string ?? UUID().uuidString
        baseRepoPath = v["baseRepoPath"]?.string ?? ""
        worktreePath = v["worktreePath"]?.string ?? ""
        branch = v["branch"]?.string
        lifecycleStatus = v["lifecycleStatus"]?.string ?? "unknown"
        dirty = v["dirty"]?.bool ?? false
        ownerKind = v["ownerKind"]?.string ?? ""
        updatedAt = v["updatedAt"]?.int ?? 0
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
