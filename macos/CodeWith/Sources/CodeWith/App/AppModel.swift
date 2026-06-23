import SwiftUI

// MARK: - Routing

enum Route: Hashable {
    case home, search, apps, loops, goals, workflows, machines, profiles
    case chat(String)      // thread id
    case project(String)   // project path
}

// MARK: - Chat message (display)

struct ChatMessage: Identifiable, Hashable {
    let id = UUID()
    enum Role { case user, assistant, tool }
    var role: Role
    var text: String
    var toolIcon: String? = nil
}

struct PendingServerRequest: Identifiable {
    enum Kind: Equatable { case commandApproval, fileChangeApproval, permissionsApproval }
    let id = UUID()
    var requestId: JSONValue
    var threadId: String?
    var method: String
    var kind: Kind
    var title: String
    var detail: String
    var requestedPermissions: JSONValue? = nil
    var actions: [PendingServerRequestAction] = []
}

struct PendingServerRequestAction: Identifiable {
    var key: String
    var title: String
    var result: JSONValue
    var isPrimary = false

    var id: String { key }
}

struct PendingUserInputRequest: Identifiable {
    let id = UUID()
    var requestId: JSONValue
    var threadId: String?
    var title: String
    var questions: [PendingUserInputQuestion]
}

struct PendingUserInputQuestion: Identifiable, Hashable {
    var id: String
    var header: String
    var question: String
    var isOther: Bool
    var isSecret: Bool
    var options: [PendingUserInputOption]
}

struct PendingUserInputOption: Identifiable, Hashable {
    var label: String
    var description: String

    var id: String { label }
}

struct PendingMcpElicitationRequest: Identifiable {
    enum Mode {
        case form
        case url(String)
    }

    let id = UUID()
    var requestId: JSONValue
    var threadId: String?
    var serverName: String
    var title: String
    var message: String
    var mode: Mode
    var fields: [PendingMcpElicitationField]
}

struct PendingMcpElicitationField: Identifiable {
    enum Kind {
        case text, secret, number, integer, singleSelect, multiSelect
    }

    var id: String
    var label: String
    var prompt: String
    var required: Bool
    var kind: Kind
    var options: [PendingMcpElicitationOption] = []
    var defaultValue: JSONValue? = nil
}

struct PendingMcpElicitationOption: Identifiable {
    var label: String
    var value: JSONValue
    var isDefault = false

    var id: String { label }
}

struct ProfileRef: Identifiable, Hashable {
    let id = UUID()
    var name: String
    var handle: String
    var plan: String
    var initials: String
    var colorHex: UInt32
}

// MARK: - App state, backed by the live app-server

@MainActor
@Observable
final class AppModel {
    enum Connection: Equatable { case connecting, connected, unavailable(String) }

    // Connection
    var connection: Connection = .connecting
    let client = AppServerClient()
    @ObservationIgnored private var notificationTasks: [Task<Void, Never>] = []

    // Navigation / UI
    var route: Route = .home
    var sidebarSelection = "New chat"
    var showSettings = false
    var settingsPage = "General"
    var composerText = ""
    var showAddMenu = false
    var showConfigPanel = false
    var planMode = false
    var fullAccess = true
    var searchQuery = ""

    // Backend data
    var threads: [ThreadInfo] = []
    var nextCursor: String? = nil
    var loadingThreads = false
    var projects: [ProjectInfo] = []
    var loops: [LoopInfo] = []
    var loopsError: String? = nil
    var goalStates: [ThreadGoalState] = []
    var goalsError: String? = nil
    var workflows: [WorkflowInfo] = []
    var workflowsError: String? = nil
    var activeGoal: GoalInfo? = nil
    var activeGoalPlans: [GoalPlanInfo] = []
    var apps: [AppItemInfo] = []
    var machines: [MachineInfo] = []
    var machinesError: String? = nil
    var selectedMachineId: String? = nil
    var machinePairing: MachinePairingInfo? = nil
    var authProfiles: [AuthProfileInfo] = []
    var activePeers: [ActiveSessionPeerInfo] = []
    var agentRuns: [AgentRunInfo] = []
    var activeAgentAttachment: AgentAttachmentInfo? = nil
    var account = AccountInfo.signedOut
    var serverVersion: String? = nil
    var serverCodewithHome: String? = nil
    var configApproval: String? = nil
    var configSandbox: String? = nil
    var configRequirements: ConfigRequirementsInfo? = nil
    var configError: String? = nil
    var desktopSettings = DesktopSettingsInfo()
    var customInstructions = ""
    var remoteSearchThreads: [ThreadInfo] = []
    private var pendingOpenURL: URL? = nil
    private var previousNonFullSandbox = "read-only"
    @ObservationIgnored private var publishedMenuBarPreference: Bool?

    // In-session config
    var model: String? = nil
    var provider: String? = nil
    var effort: String = "Low"
    var permissionProfileId = ":danger-full-access"
    var availableModels = ["gpt-5.5-codex", "gpt-5.5", "o3", "gpt-4.1"]
    var availableProviders = ["openai", "azure", "openrouter", "ollama"]
    let availablePermissionProfiles = [":read-only", ":workspace", ":danger-full-access"]
    let availableEfforts = ["Low", "Medium", "High", "Extra High"]

    // Active chat
    var activeThreadId: String? = nil
    var activeTurnId: String? = nil
    var activeTurnThreadId: String? = nil
    var activeMessages: [ChatMessage] = []
    var pendingServerRequests: [PendingServerRequest] = []
    var pendingUserInputRequests: [PendingUserInputRequest] = []
    var pendingMcpElicitationRequests: [PendingMcpElicitationRequest] = []
    var pendingActivePeer: ActiveSessionPeerInfo? = nil
    var turnInProgress = false
    var currentProjectPath: String? = nil
    private var streamingAssistantIndex: Int? = nil
    @ObservationIgnored private var toolOutputMessageIndexes: [String: Int] = [:]
    @ObservationIgnored private var respondingServerRequestKeys = Set<String>()
    private var resumedThreadIds = Set<String>()

    // Turn watchdog: if a turn goes silent (no deltas/items/completion) for this
    // long, the agent is presumed stuck and we release the spinner with an error.
    @ObservationIgnored private var turnWatchdog: Task<Void, Never>? = nil
    @ObservationIgnored private var lastTurnActivity = Date()
    private static let turnSilenceTimeout: TimeInterval = 300

    // Profiles (local switch; profile picker UI)
    var profiles: [ProfileRef]
    var currentProfileID: UUID

    init() {
        let me = ProfileRef(name: "You", handle: "@me", plan: "", initials: "ME", colorHex: 0x4AB58E)
        profiles = [me]
        currentProfileID = me.id
        installExitHandler()
    }

    private func installExitHandler() {
        client.onExit = { [weak self] _ in
            Task { @MainActor in
                guard let self else { return }
                self.connection = .unavailable("app-server stopped")
                self.turnInProgress = false
                self.clearPendingServerInteractions()
            }
        }
    }

    func clearPendingServerInteractions() {
        pendingServerRequests = []
        pendingUserInputRequests = []
        pendingMcpElicitationRequests = []
        respondingServerRequestKeys.removeAll()
    }

    private func beginServerRequestResponse(to requestId: JSONValue) -> Bool {
        let key = Self.serverRequestKey(requestId)
        if respondingServerRequestKeys.contains(key) { return false }
        respondingServerRequestKeys.insert(key)
        return true
    }

    private static func serverRequestKey(_ requestId: JSONValue) -> String {
        switch requestId {
        case .null:
            return "null"
        case .bool(let value):
            return "bool:\(value)"
        case .number(let value):
            return "number:\(value)"
        case .string(let value):
            return "string:\(value)"
        case .array, .object:
            let data = (try? JSONEncoder().encode(requestId)) ?? Data()
            return "json:\(String(data: data, encoding: .utf8) ?? String(describing: requestId))"
        }
    }

    /// Consume server notifications in arrival order on the main actor.
    private func startNotificationConsumer() {
        notificationTasks.forEach { $0.cancel() }
        notificationTasks.removeAll()
        notificationTasks.append(Task { @MainActor [weak self] in
            guard let stream = self?.client.notifications else { return }
            for await (method, params) in stream {
                guard !Task.isCancelled else { return }
                self?.handleNotification(method: method, params: params)
            }
        })
        notificationTasks.append(Task { @MainActor [weak self] in
            guard let stream = self?.client.serverRequests else { return }
            for await request in stream {
                guard !Task.isCancelled else { return }
                self?.handleServerRequest(request)
            }
        })
    }

    func shutdown() {
        notificationTasks.forEach { $0.cancel() }
        notificationTasks.removeAll()
        client.onExit = nil
        clearPendingServerInteractions()
        client.stop()
    }

    func reconnectAppServer() async {
        shutdown()
        turnInProgress = false
        activeTurnId = nil
        clearPendingServerInteractions()
        cancelTurnWatchdog()
        connection = .connecting
        installExitHandler()
        await bootstrap()
    }

    var currentProfile: ProfileRef { profiles.first { $0.id == currentProfileID } ?? profiles[0] }

    // MARK: Bootstrap

    func bootstrap() async {
        guard connection == .connecting else { return }   // idempotent
        // Try each resolvable CLI candidate in order and keep the first that both
        // starts AND completes the initialize handshake. This makes the app
        // resilient to a broken candidate (e.g. a bundled CLI that exits on spawn
        // because it lost its node_modules) by falling through to the next, such
        // as the system /opt/homebrew/bin/codewith.
        let candidates = AppServerClient.candidateBinaries.filter {
            FileManager.default.isExecutableFile(atPath: $0) && !AppServerClient.isSelfExecutable($0)
        }
        guard !candidates.isEmpty else {
            connection = .unavailable("codewith CLI not found"); return
        }
        var lastError = "could not start the codewith app-server"
        for candidate in candidates {
            do {
                try client.start(binary: candidate)
                let initResult = try await client.initialize()
                serverVersion = Self.parseVersion(initResult["userAgent"]?.string)
                serverCodewithHome = initResult["codexHome"]?.string
                connection = .connected
                break
            } catch {
                lastError = error.localizedDescription
                client.stop()   // tear down the dead process before trying the next
            }
        }
        guard connection == .connected else {
            connection = .unavailable(lastError); return
        }
        startNotificationConsumer()
        await refreshAll()
        if let pendingOpenURL {
            self.pendingOpenURL = nil
            await openDesktopURL(pendingOpenURL)
        }
    }

    func refreshAll() async {
        async let acct: () = loadAccount()
        async let apps: () = loadApps()
        async let machines: () = loadMachines()
        async let peers: () = loadActivePeers()
        async let agents: () = loadAgentRuns()
        async let profiles: () = loadProfiles()
        async let requirements: () = loadConfigRequirements()
        await loadConfig()
        async let catalog: () = loadModelCatalog()
        await loadThreads(reset: true)          // fast first-page paint
        await loadLoops()
        await loadGoals()
        await loadWorkflows()
        _ = await (acct, apps, machines, peers, agents, profiles, requirements, catalog)
        // Drain remaining pages in the background so Projects becomes complete.
        Task { [weak self] in
            guard let self else { return }
            var guardCount = 0
            while await self.nextCursor != nil, guardCount < 60 {
                await self.loadThreads(reset: false)
                guardCount += 1
            }
            await self.loadLoops()
            await self.loadGoals()
            await self.loadWorkflows()
        }
    }

    var addMenuAgentRuns: [AgentRunInfo] {
        let activeThreadIds = Set(activePeers.map(\.threadId).filter { !$0.isEmpty })
        return agentRuns.filter { agent in
            agent.canOpenThread && !agent.isDeleted && !activeThreadIds.contains(agent.threadId)
        }
    }

    func loadApps() async {
        guard connection == .connected else { return }
        apps = (try? await client.listApps()) ?? []
    }

    func loadModelCatalog() async {
        guard connection == .connected else { return }
        if let providers = try? await client.listModelProviders(), !providers.isEmpty {
            availableProviders = providers
            if provider == nil || !providers.contains(provider ?? "") {
                provider = providers.first
            }
        }
        if let models = try? await client.listModels(provider: provider), !models.isEmpty {
            availableModels = models
            if model == nil || !models.contains(model ?? "") {
                model = models.first
            }
        }
    }

    func loadMachines() async {
        guard connection == .connected else { return }
        do {
            machines = try await client.listMachines()
            if selectedMachineId == nil {
                selectedMachineId = machines.first(where: \.isLocal)?.machineId ?? machines.first?.machineId
            }
            machinesError = nil
            refreshProjects()
        } catch {
            machines = []
            // Older app-servers don't implement the machine registry; show a short
            // note rather than dumping the raw "unknown variant `machineRegistry/list`"
            // JSON-RPC error (e.g. the codewith build on apple01 predates it).
            machinesError = Self.isUnsupportedMethodError(error)
                ? "this app-server version doesn't expose a machine registry."
                : error.localizedDescription
        }
    }

    /// True when the app-server rejected the request as an unknown/unsupported
    /// method (JSON-RPC -32600 "unknown variant ..."), i.e. the connected build
    /// predates the feature being called.
    static func isUnsupportedMethodError(_ error: Error) -> Bool {
        if case let AppServerError.rpc(code, message) = error {
            return code == -32600 || message.localizedCaseInsensitiveContains("unknown variant")
        }
        return false
    }

    func startMachinePairing() async {
        guard connection == .connected else { return }
        machinesError = nil
        do {
            machinePairing = try await client.startMachinePairing()
            machinesError = nil
        } catch {
            machinePairing = nil
            machinesError = error.localizedDescription
        }
    }

    func refreshMachinePairingStatus() async {
        guard connection == .connected, let pairing = machinePairing else { return }
        do {
            if try await client.machinePairingClaimed(pairing) {
                machinePairing = nil
                machinesError = nil
                await loadMachines()
            } else {
                machinesError = "Machine pairing has not been claimed yet."
            }
        } catch {
            machinesError = error.localizedDescription
        }
    }

    func loadActivePeers() async {
        guard connection == .connected else { return }
        if let peers = try? await client.listActiveSessions() {
            activePeers = peers.filter { peer in
                let isCurrentThread = peer.threadId == activeThreadId
                    || (peer.threadId.isEmpty && peer.peerId == activeThreadId)
                return !isCurrentThread && peer.canReceiveMessage
            }
        }
    }

    func loadAgentRuns() async {
        guard connection == .connected else { return }
        if let runs = try? await client.listAgentRuns() {
            agentRuns = runs
        }
    }

    func toggleLoop(_ loop: LoopInfo) async {
        guard connection == .connected, !loop.threadId.isEmpty, loop.canToggle else { return }
        // Optimistic flip, then call backend and reload.
        let index = loops.firstIndex(where: { $0.id == loop.id })
        if let index { loops[index].active.toggle() }
        do {
            try await client.setLoopActive(loop, active: !loop.active)
            loopsError = nil
        } catch {
            if let index { loops[index].active = loop.active }
            loopsError = error.localizedDescription
        }
        await loadLoops()
    }

    func createDefaultLoop() async {
        guard connection == .connected else { return }
        let prompt = composerText.trimmingCharacters(in: .whitespacesAndNewlines)
        let loopPrompt = prompt.isEmpty ? LoopCreationDraft.defaultPrompt : prompt
        var draft = LoopCreationDraft()
        draft.prompt = loopPrompt
        await createLoop(draft, fallbackToComposer: true)
    }

    func createLoop(_ draft: LoopCreationDraft, fallbackToComposer: Bool = false) async {
        guard connection == .connected else { return }
        guard draft.canCreate else {
            loopsError = draft.validationMessage
            return
        }
        guard let tid = activeThreadId else {
            if fallbackToComposer {
                composerText = "Loop: \(draft.normalizedPrompt)"
            } else {
                loopsError = "Open a chat before creating a loop."
            }
            return
        }
        do {
            switch draft.kind {
            case .schedule:
                _ = try await client.createSchedule(
                    threadId: tid,
                    prompt: draft.normalizedPrompt,
                    promptSource: "inline",
                    schedule: Self.loopScheduleSpec(from: draft)
                )
            case .monitor:
                _ = try await client.createMonitor(
                    threadId: tid,
                    name: draft.normalizedMonitorName,
                    prompt: draft.normalizedPrompt,
                    command: draft.normalizedCommand,
                    cwd: draft.normalizedCwd,
                    routing: draft.routing.rawValue,
                    outputFile: draft.normalizedOutputFile
                )
            }
            loopsError = nil
            composerText = ""
            await loadLoops()
        } catch {
            loopsError = error.localizedDescription
            if fallbackToComposer {
                composerText = "Loop: \(draft.normalizedPrompt)"
            }
        }
    }

    static func loopScheduleSpec(from draft: LoopCreationDraft) -> JSONValue {
        switch draft.scheduleMode {
        case .dynamic:
            return AppServerClient.dynamicScheduleSpec()
        case .interval:
            return AppServerClient.intervalScheduleSpec(
                amount: draft.intervalAmountValue ?? 1,
                unit: draft.intervalUnit
            )
        case .cron:
            return AppServerClient.cronScheduleSpec(expression: draft.normalizedCronExpression)
        }
    }

    func deleteLoop(_ loop: LoopInfo) async {
        guard connection == .connected, !loop.threadId.isEmpty else { return }
        do {
            let deleted = try await client.deleteLoop(loop)
            loopsError = deleted ? nil : "Loop was not deleted."
        } catch {
            loopsError = error.localizedDescription
        }
        await loadLoops()
    }

    func runLoopNow(_ loop: LoopInfo) async {
        guard connection == .connected, loop.canRunNow else { return }
        do {
            let started = try await client.runLoopNow(loop)
            loopsError = started ? nil : "Loop did not start."
        } catch {
            loopsError = error.localizedDescription
        }
    }

    // MARK: Threads / projects

    func loadThreads(reset: Bool) async {
        guard connection == .connected, !loadingThreads else { return }
        loadingThreads = true
        defer { loadingThreads = false }
        do {
            let cursor = reset ? nil : nextCursor
            // Load a larger first page so Projects (derived from cwd) are complete.
            let (newThreads, next) = try await client.listThreads(cursor: cursor, limit: reset ? 200 : 50)
            if reset { threads = newThreads } else { threads.append(contentsOf: newThreads) }
            nextCursor = next
            refreshProjects()
        } catch {
            // keep existing data; surface nothing fatal
        }
    }

    func loadMoreThreads() async {
        guard nextCursor != nil else { return }
        await loadThreads(reset: false)
    }

    /// Drain every page so Projects (derived from cwd/repo) is complete.
    func loadAllProjects() async {
        await loadThreads(reset: true)
        var guardCount = 0
        while nextCursor != nil, guardCount < 60 {
            await loadThreads(reset: false)
            guardCount += 1
        }
    }

    var hasMoreThreads: Bool { nextCursor != nil }

    var selectedMachine: MachineInfo? {
        guard let selectedMachineId else { return nil }
        return machines.first { $0.machineId == selectedMachineId }
    }

    var currentMachineLabel: String {
        selectedMachine?.displayName ?? "This machine"
    }

    var machineScopedThreads: [ThreadInfo] {
        guard let selectedMachineId else { return threads }
        let scoped = threads.filter { $0.machineId == selectedMachineId }
        if !scoped.isEmpty { return scoped }
        // Older app-server thread metadata does not carry machine ids. Keep the
        // selected-machine UI useful by falling back to the connected app-server's
        // local thread list instead of showing an empty project tree.
        let anyMachineMetadata = threads.contains { $0.machineId != nil }
        return anyMachineMetadata ? [] : threads
    }

    func selectMachine(_ machine: MachineInfo?) {
        selectedMachineId = machine?.machineId
        currentProjectPath = nil
        refreshProjects()
        Task {
            await loadLoops()
            await loadGoals()
            await loadWorkflows()
        }
    }

    private func refreshProjects() {
        projects = ProjectInfo.derive(from: machineScopedThreads)
    }

    // MARK: Search (local, over loaded data)

    private var q: String { searchQuery.trimmingCharacters(in: .whitespaces).lowercased() }
    var searchThreads: [ThreadInfo] {
        if q.isEmpty { return [] }
        return remoteSearchThreads.isEmpty
            ? machineScopedThreads.filter { $0.name.lowercased().contains(q) }
            : remoteSearchThreads
    }
    var searchProjects: [ProjectInfo] {
        q.isEmpty ? [] : projects.filter { $0.name.lowercased().contains(q) }
    }
    var searchApps: [AppItemInfo] {
        q.isEmpty ? [] : apps.filter { $0.name.lowercased().contains(q) || $0.detail.lowercased().contains(q) }
    }
    var hasSearchResults: Bool { !searchThreads.isEmpty || !searchProjects.isEmpty || !searchApps.isEmpty }

    func runSearch() async {
        let term = searchQuery.trimmingCharacters(in: .whitespacesAndNewlines)
        guard connection == .connected, !term.isEmpty else {
            remoteSearchThreads = []
            return
        }
        let results = (try? await client.searchThreads(term: term)) ?? []
        guard searchQuery.trimmingCharacters(in: .whitespacesAndNewlines) == term else { return }
        remoteSearchThreads = results
    }

    func loadLoops() async {
        guard connection == .connected else { return }
        // Schedules/monitors are per-thread; aggregate across loaded threads.
        do {
            let loadedLoops = try await client.listLoops(threadIds: machineScopedThreads.map(\.id))
            loops = loadedLoops
            loopsError = nil
        } catch {
            loopsError = error.localizedDescription
        }
    }

    func loadGoals() async {
        guard connection == .connected else { return }
        do {
            goalStates = try await client.listThreadGoalStates(threadIds: machineScopedThreads.map(\.id))
            goalsError = nil
        } catch {
            goalsError = error.localizedDescription
        }
    }

    func loadWorkflows() async {
        guard connection == .connected else { return }
        do {
            workflows = try await client.listWorkflows(threadIds: machineScopedThreads.map(\.id))
            workflowsError = nil
        } catch {
            workflowsError = Self.isUnsupportedMethodError(error)
                ? "this app-server version doesn't expose workflows."
                : error.localizedDescription
        }
    }

    func loadActiveGoal() async {
        guard connection == .connected, let activeThreadId else {
            activeGoal = nil
            activeGoalPlans = []
            return
        }
        do {
            let state = try await client.listThreadGoalState(threadId: activeThreadId)
            activeGoal = state.goal
            activeGoalPlans = state.goalPlans
        } catch {
            activeGoal = nil
            activeGoalPlans = []
        }
    }

    func activateGoalPlanNode(_ node: GoalPlanNodeInfo) async {
        guard connection == .connected else { return }
        let threadId = node.threadId.isEmpty ? activeThreadId : node.threadId
        guard let threadId, !threadId.isEmpty, node.canActivate else { return }
        do {
            let state = try await client.activateGoalPlanNode(threadId: threadId, nodeId: node.nodeId)
            if state.threadId == activeThreadId {
                activeGoal = state.goal ?? activeGoal
                for plan in state.goalPlans {
                    upsertActiveGoalPlan(plan)
                }
            }
            await loadActiveGoal()
        } catch {
            activeMessages.append(ChatMessage(role: .tool, text: "Goal plan update failed: \(error.localizedDescription)", toolIcon: "exclamationmark.triangle"))
        }
    }

    func loadAccount() async {
        if let a = try? await client.readAccount() { account = a }
    }

    func loadConfig() async {
        do {
            let cfg = try await client.readFullConfig()
            if let configModel = cfg.model { model = configModel }
            if let configProvider = cfg.provider { provider = configProvider }
            if let wireEffort = cfg.effort { effort = Self.displayEffort(wireEffort) }
            configApproval = cfg.approval
            configSandbox = cfg.sandbox
            customInstructions = cfg.developerInstructions ?? ""
            desktopSettings = cfg.desktop
            publishMenuBarPreference(cfg.desktop.showMenuBar)
            fullAccess = cfg.sandbox == "danger-full-access"
            permissionProfileId = Self.permissionProfileId(forSandbox: cfg.sandbox)
            if let sandbox = cfg.sandbox, sandbox != "danger-full-access" {
                previousNonFullSandbox = sandbox
            }
        } catch {
            configError = error.localizedDescription
        }
    }

    func loadConfigRequirements() async {
        guard connection == .connected else { return }
        do {
            configRequirements = try await client.readConfigRequirements()
        } catch {
            configError = error.localizedDescription
        }
    }

    var approvalOptions: [String] {
        configRequirements?.approvalOptions(defaults: ["untrusted", "on-failure", "on-request", "never"])
            ?? ["untrusted", "on-failure", "on-request", "never"]
    }

    var sandboxOptions: [String] {
        configRequirements?.sandboxOptions(defaults: ["read-only", "workspace-write", "danger-full-access"])
            ?? ["read-only", "workspace-write", "danger-full-access"]
    }

    var canUseFullAccess: Bool {
        configRequirements?.allowsSandbox("danger-full-access") ?? true
    }

    func loadProfiles() async {
        guard connection == .connected else { return }
        if let profiles = try? await client.listAuthProfiles() {
            authProfiles = profiles
        }
    }

    // MARK: Login / auth

    var loginInProgress = false
    var loginError: String? = nil
    private var pendingLoginId: String? = nil

    var isSignedIn: Bool {
        if !account.requiresOpenAIAuth { return true }
        let n = account.name
        return n != "Signed out" && !n.isEmpty
    }

    /// Start ChatGPT OAuth: open the returned auth URL in the browser. The
    /// `account/login/completed` notification finalizes it.
    func loginWithChatGPT() async {
        guard connection == .connected, !loginInProgress else { return }
        loginInProgress = true; loginError = nil
        do {
            let r = try await client.request("account/login/start", .object(["type": .string("chatgpt")]), timeout: 30)
            pendingLoginId = r["loginId"]?.string
            guard let url = Self.loginURL(from: r) else {
                loginError = "No login URL was returned."
                pendingLoginId = nil
                loginInProgress = false
                return
            }
            #if canImport(AppKit)
            if !NSWorkspace.shared.open(url) {
                loginError = "Could not open the login URL."
                pendingLoginId = nil
                loginInProgress = false
            }
            #else
            loginError = "Cannot open the login URL on this platform."
            pendingLoginId = nil
            loginInProgress = false
            #endif
        } catch {
            loginError = error.localizedDescription
            loginInProgress = false
        }
    }

    static func loginURL(from response: JSONValue) -> URL? {
        for key in ["authUrl", "verificationUrl"] {
            if let value = response[key]?.string, let url = URL(string: value) {
                return url
            }
        }
        return nil
    }

    /// Sign in with an OpenAI API key.
    func loginWithApiKey(_ key: String, providerName: String = "OpenAI") async {
        let k = key.trimmingCharacters(in: .whitespacesAndNewlines)
        guard connection == .connected, !loginInProgress, !k.isEmpty else { return }
        guard Self.providerID(for: providerName) == "openai" else {
            await loginWithoutApiKey(providerName: providerName)
            return
        }
        loginInProgress = true; loginError = nil
        do {
            try await client.writeConfig(keyPath: "model_provider", value: .string(Self.providerID(for: providerName)))
            _ = try await client.request("account/login/start",
                .object(["type": .string("apiKey"), "apiKey": .string(k)]), timeout: 30)
            await loadConfig()
            await loadModelCatalog()
            await loadAccount()
            if !isSignedIn { loginError = "That key was not accepted." }
        } catch {
            loginError = error.localizedDescription
        }
        loginInProgress = false
    }

    func loginWithoutApiKey(providerName: String) async {
        guard connection == .connected, !loginInProgress else { return }
        loginInProgress = true; loginError = nil
        do {
            try await client.writeConfig(keyPath: "model_provider", value: .string(Self.providerID(for: providerName)))
            await loadConfig()
            await loadModelCatalog()
            await loadAccount()
        } catch {
            loginError = error.localizedDescription
        }
        loginInProgress = false
        if loginError == nil, !isSignedIn {
            loginError = "\(providerName) is not ready. Check your provider configuration."
        }
    }

    func cancelLogin() async {
        if let pendingLoginId {
            _ = try? await client.request("account/login/cancel", .object(["loginId": .string(pendingLoginId)]), timeout: 10)
        }
        pendingLoginId = nil
        loginInProgress = false
    }

    func logout() async {
        _ = try? await client.request("account/logout", timeout: 10)
        await loadAccount()
    }

    /// Switch the active CLI profile, then reconnect the session.
    func switchAuthProfile(_ name: String) async {
        guard connection == .connected else { return }
        do {
            _ = try await client.switchAuthProfile(name)
            await reconnectAppServer()
            await loadProfiles()
        } catch {
            await loadProfiles()
        }
    }

    static func parseVersion(_ userAgent: String?) -> String? {
        guard let ua = userAgent else { return nil }
        // Extract the first X.Y.Z token.
        var current = ""
        for ch in ua {
            if ch.isNumber || ch == "." { current.append(ch) }
            else {
                if current.filter({ $0 == "." }).count >= 2 { return current }
                current = ""
            }
        }
        return current.filter { $0 == "." }.count >= 2 ? current : nil
    }

    static func providerID(for displayName: String) -> String {
        switch displayName.lowercased() {
        case "openrouter": return "openrouter"
        case "azure": return "azure"
        case "anthropic": return "anthropic"
        case "ollama": return "ollama"
        default: return "openai"
        }
    }

    // MARK: Navigation

    func open(_ r: Route, label: String) {
        route = r; sidebarSelection = label; showSettings = false; showAddMenu = false
    }

    func openThread(_ t: ThreadInfo) async {
        let requestedThreadId = t.id
        activeThreadId = t.id
        if activeTurnThreadId != t.id {
            activeTurnId = nil
        }
        activeMessages = []
        toolOutputMessageIndexes.removeAll()
        pendingActivePeer = nil
        activeAgentAttachment = nil
        activeGoal = nil
        activeGoalPlans = []
        currentProjectPath = t.cwd
        route = .chat(t.id); sidebarSelection = t.name; showSettings = false
        // Prefer resume (so future turns can continue the thread); fall back to a
        // plain read if resume isn't available. `await` can't live in a `??`
        // autoclosure, so branch explicitly.
        let messages: [ChatMessage]
        if let resumed = try? await client.resumeThreadMessages(id: t.id) {
            messages = resumed
            resumedThreadIds.insert(t.id)
        } else {
            messages = (try? await client.readThreadMessages(id: t.id)) ?? []
            resumedThreadIds.remove(t.id)
        }
        guard activeThreadId == requestedThreadId else { return }
        activeMessages = messages
        await loadActiveGoal()
        await loadActivePeers()
        await loadAgentRuns()
    }

    func openSettings(_ page: String = "General") { showSettings = true; settingsPage = page }

    func handleDesktopURL(_ url: URL) {
        guard connection == .connected else {
            pendingOpenURL = url
            return
        }
        Task { await openDesktopURL(url) }
    }

    private func openDesktopURL(_ url: URL) async {
        guard url.scheme == "codewith" || url.scheme == "codex" else { return }
        let components = URLComponents(url: url, resolvingAgainstBaseURL: false)
        let host = url.host ?? ""
        let pathParts = url.path.split(separator: "/").map(String.init)
        guard host == "threads" else { return }

        if pathParts.first == "new" {
            let path = components?.queryItems?.first { $0.name == "path" }?.value
            if let path, !path.isEmpty { newSessionInProject(path) }
            else { newChat() }
            return
        }

        if let threadId = pathParts.first, !threadId.isEmpty {
            let thread = threads.first { $0.id == threadId }
                ?? ThreadInfo(from: .object(["id": .string(threadId), "name": .string("Chat")]))
            await openThread(thread)
        }
    }

    /// Show a project's sessions (and let the user start a new one there).
    func openProject(_ p: ProjectInfo) {
        currentProjectPath = p.path
        open(.project(p.groupKey), label: p.name)
    }

    /// New session scoped to a project's directory.
    func newSessionInProject(_ path: String) {
        currentProjectPath = path
        composerText = ""; activeThreadId = nil; activeTurnId = nil; activeGoal = nil; activeGoalPlans = []; activeMessages = []; pendingActivePeer = nil; activeAgentAttachment = nil
        toolOutputMessageIndexes.removeAll()
        open(.home, label: (path as NSString).lastPathComponent)
    }

    /// Threads belonging to a project, by repo-identity group key.
    func threads(forProjectKey key: String) -> [ThreadInfo] {
        machineScopedThreads.filter { ($0.projectKey ?? $0.cwd ?? "") == key }
    }
    func project(forKey key: String) -> ProjectInfo? {
        projects.first { $0.groupKey == key }
    }

    /// The label for the header project selector.
    var currentProjectLabel: String {
        guard let path = currentProjectPath else { return "All projects" }
        return projects.first { $0.path == path }?.name ?? (path as NSString).lastPathComponent
    }

    /// Select the project context for new sessions (nil = all projects on the selected machine).
    func selectProject(_ p: ProjectInfo?) {
        if activeThreadId != nil {
            if let p {
                newSessionInProject(p.path)
            } else {
                newChat()
            }
            return
        }
        currentProjectPath = p?.path
    }

    func newChat() {
        composerText = ""; activeThreadId = nil; activeTurnId = nil; activeGoal = nil; activeGoalPlans = []; activeMessages = []; pendingActivePeer = nil; activeAgentAttachment = nil
        toolOutputMessageIndexes.removeAll()
        currentProjectPath = nil
        open(.home, label: "New chat")
    }

    // MARK: Sending

    func submitComposer() async {
        let text = composerText.trimmingCharacters(in: .whitespacesAndNewlines)
        // Guard against a second submit while a turn is already running (prevents
        // the same prompt being sent twice via Enter + button or rapid repeats).
        guard !text.isEmpty, connection == .connected, !turnInProgress else { return }
        if Self.isGoalCommand(text), Self.goalObjective(from: text).isEmpty {
            composerText = "Goal: "
            return
        }
        if Self.isLoopCommand(text), Self.loopPrompt(from: text).isEmpty {
            composerText = "Loop: "
            return
        }
        if let pendingActivePeer {
            let message = Self.activePeerMessage(from: text, peer: pendingActivePeer)
            guard !message.isEmpty else {
                composerText = "@\(pendingActivePeer.displayName) "
                return
            }
            composerText = message
            await sendComposerToActivePeer(pendingActivePeer)
            return
        }
        turnInProgress = true   // set immediately to block re-entry across the awaits below
        streamingAssistantIndex = nil
        activeTurnId = nil
        composerText = ""
        // Show the user's message immediately for responsiveness.
        activeMessages.append(ChatMessage(role: .user, text: text))
        guard let tid = await ensureActiveThread() else {
            finishTurn(failureMessage: "Couldn't start a session. Is the app-server connected?")
            return
        }
        if Self.isGoalCommand(text) {
            let objective = Self.goalObjective(from: text)
            if !objective.isEmpty {
                do {
                    activeGoal = try await client.setThreadGoal(threadId: tid, objective: objective)
                    activeMessages.append(ChatMessage(role: .tool, text: "Set goal: \(objective)", toolIcon: "target"))
                    route = .chat(tid)
                    turnInProgress = false
                    await loadThreads(reset: true)
                    return
                } catch {
                    finishTurn(failureMessage: error.localizedDescription)
                    return
                }
            }
        }
        if Self.isLoopCommand(text) {
            let prompt = Self.loopPrompt(from: text)
            do {
                _ = try await client.createSchedule(
                    threadId: tid,
                    prompt: prompt,
                    promptSource: "inline",
                    schedule: AppServerClient.dynamicScheduleSpec()
                )
                activeMessages.append(ChatMessage(role: .tool, text: "Created loop: \(prompt)", toolIcon: "clock.arrow.circlepath"))
                route = .chat(tid)
                turnInProgress = false
                activeTurnThreadId = nil
                await loadLoops()
                await loadThreads(reset: true)
                return
            } catch {
                finishTurn(failureMessage: error.localizedDescription)
                return
            }
        }
        route = .chat(tid)
        activeTurnThreadId = tid
        startTurnWatchdog()
        do {
            let wireEffort = Self.wireEffort(effort)
            let collaborationMode = planMode
                ? AppServerClient.planCollaborationMode(model: model, effort: wireEffort)
                : nil
            let turnId = try await client.startTurn(threadId: tid, input: text, model: model, provider: provider,
                                                    effort: wireEffort, collaborationMode: collaborationMode)
            if collaborationMode != nil { planMode = false }
            activeTurnId = turnId
        } catch {
            finishTurn(failureMessage: error.localizedDescription)
        }
    }

    private func ensureActiveThread() async -> String? {
        if let activeThreadId {
            if resumedThreadIds.contains(activeThreadId) { return activeThreadId }
            if (try? await client.resumeThreadMessages(id: activeThreadId)) != nil {
                resumedThreadIds.insert(activeThreadId)
                return activeThreadId
            }
            return nil
        }
        activeThreadId = try? await client.startThread(cwd: currentProjectPath ?? NSHomeDirectory())
        if let activeThreadId {
            resumedThreadIds.insert(activeThreadId)
            await loadThreads(reset: true)
        }
        return activeThreadId
    }

    func interrupt() async {
        guard let tid = activeTurnThreadId ?? activeThreadId, let turnId = activeTurnId else { return }
        await client.interruptTurn(threadId: tid, turnId: turnId)
        finishTurn(failureMessage: nil)
    }

    /// Returns a user-facing failure message iff the turn payload signals failure.
    static func turnFailureMessage(_ params: JSONValue) -> String? {
        guard let turn = params["turn"], turn["status"]?.string == "failed" else { return nil }
        return turn["error"]?["message"]?.string ?? "The turn failed."
    }

    /// Release the turn-in-progress state, cancel the watchdog, and optionally
    /// surface a failure message. Idempotent.
    func finishTurn(failureMessage: String?) {
        turnInProgress = false
        streamingAssistantIndex = nil
        activeTurnId = nil
        activeTurnThreadId = nil
        cancelTurnWatchdog()
        if let failureMessage {
            activeMessages.append(ChatMessage(role: .assistant, text: "⚠︎ \(failureMessage)"))
        }
    }

    private func noteTurnActivity() { lastTurnActivity = Date() }

    /// Start (or restart) the silence watchdog for the active turn. If no
    /// streaming activity arrives for `turnSilenceTimeout`, the turn is presumed
    /// stuck and the spinner is released with an explanatory message.
    private func startTurnWatchdog() {
        cancelTurnWatchdog()
        lastTurnActivity = Date()
        let threadAtArm = activeTurnThreadId ?? activeThreadId
        turnWatchdog = Task { @MainActor [weak self] in
            while true {
                try? await Task.sleep(nanoseconds: 5 * 1_000_000_000)
                guard let self, !Task.isCancelled else { return }
                guard self.turnInProgress, self.activeTurnThreadId == threadAtArm else { return }
                if Date().timeIntervalSince(self.lastTurnActivity) >= Self.turnSilenceTimeout {
                    let visibleFailure = self.activeThreadId == threadAtArm
                        ? "The agent didn't respond in time. It may be stuck — try sending again."
                        : nil
                    self.finishTurn(failureMessage: visibleFailure)
                    return
                }
            }
        }
    }

    private func cancelTurnWatchdog() {
        turnWatchdog?.cancel()
        turnWatchdog = nil
    }

    // MARK: Live notifications (turn streaming)

    func handleNotification(method: String, params: JSONValue) {
        switch method {
        // Real wire name is "item/agentMessage/delta"; aliases kept defensively.
        case "item/agentMessage/delta", "item/agentMessageDelta", "agentMessageDelta", "thread/agentMessageDelta":
            guard notificationBelongsToActiveThread(params) else { return }
            noteTurnActivity()
            appendAssistantDelta(params["delta"]?.string ?? params["text"]?.string ?? "")
        case "item/completed", "thread/item/completed", "thread/realtimeItemAdded":
            guard notificationBelongsToActiveThread(params) else { return }
            noteTurnActivity()
            handleCompletedItem(params["item"] ?? .null)
        case "item/commandExecution/outputDelta", "command/exec/outputDelta", "process/outputDelta":
            guard notificationBelongsToActiveTurn(params) else { return }
            noteTurnActivity()
            appendToolOutputDelta(method: method, params: params)
        case "item/commandExecution/terminalInteraction":
            guard notificationBelongsToActiveTurn(params) else { return }
            noteTurnActivity()
            appendTerminalInteraction(params)
        case "turn/started", "thread/turn/started":
            guard notificationBelongsToActiveTurn(params) else { return }
            turnInProgress = true
            activeTurnId = params["turn"]?["id"]?.string ?? params["turnId"]?.string ?? activeTurnId
            startTurnWatchdog()
        case "turn/completed", "thread/turn/completed":
            guard notificationBelongsToActiveTurn(params) else { return }
            finishTurn(failureMessage: notificationIsForActiveThread(params) ? Self.turnFailureMessage(params) : nil)
        case "turn/failed", "thread/turn/failed":
            // Not a real wire method per the schema, but handle defensively.
            guard notificationBelongsToActiveTurn(params) else { return }
            finishTurn(failureMessage: notificationIsForActiveThread(params) ? (Self.turnFailureMessage(params) ?? "The turn failed.") : nil)
        case "error", "thread/error":
            // Retryable errors stream with willRetry:true; surface only terminal ones.
            guard notificationBelongsToActiveThread(params) else { return }
            noteTurnActivity()
            if params["willRetry"]?.bool != true,
               let msg = params["error"]?["message"]?.string ?? params["message"]?.string {
                activeMessages.append(ChatMessage(role: .assistant, text: "⚠︎ \(msg)"))
            }
        case "account/login/completed", "account/updated", "account/login/chatGptComplete":
            loginInProgress = false
            pendingLoginId = nil
            Task { await loadAccount(); await refreshAll() }
        case "thread/started", "thread/closed", "thread/archived", "thread/unarchived":
            // A session appeared/changed — refresh the list so Projects + Chats stay live.
            Task {
                await loadThreads(reset: true)
                await loadLoops()
                await loadGoals()
                await loadWorkflows()
                await loadActivePeers()
                await loadAgentRuns()
            }
        case "thread/name/updated", "thread/status/changed", "thread/metadata/updated":
            Task {
                await loadThreads(reset: true)
                await loadGoals()
                await loadWorkflows()
                await loadActivePeers()
                await loadAgentRuns()
            }
        case "thread/settings/updated":
            applyThreadSettingsNotification(params)
            Task { await loadModelCatalog() }
        case "thread/schedule/updated", "thread/schedule/deleted",
             "thread/monitor/updated", "thread/monitor/deleted":
            Task { await loadLoops() }
        case "thread/goal/updated":
            guard notificationBelongsToActiveThread(params) else { return }
            if let goal = params["goal"], !goal.isNull {
                activeGoal = GoalInfo(from: goal)
            } else {
                Task { await loadActiveGoal() }
            }
            Task { await loadGoals() }
        case "thread/goalPlan/updated":
            guard notificationBelongsToActiveThread(params) else { return }
            if let plan = params["plan"], !plan.isNull {
                upsertActiveGoalPlan(GoalPlanInfo(from: plan))
            } else {
                Task { await loadActiveGoal() }
            }
            Task { await loadGoals() }
        case "thread/goal/cleared":
            guard notificationBelongsToActiveThread(params) else { return }
            activeGoal = nil
            activeGoalPlans = []
            Task { await loadGoals() }
        case "thread/workflow/updated", "thread/workflow/deleted",
             "thread/workflow/run/updated", "thread/workflow/run/deleted":
            Task { await loadWorkflows() }
        case "app/list/updated":
            let updated = AppServerClient.parseApps(params["data"]?.array ?? [])
            if updated.isEmpty { Task { await loadApps() } } else { apps = updated }
        case "serverRequest/resolved":
            let resolvedId = params["requestId"] ?? params["id"]
            let resolvedThreadId = params["threadId"]?.string
            if let resolvedId {
                respondingServerRequestKeys.remove(Self.serverRequestKey(resolvedId))
                pendingServerRequests.removeAll { request in
                    request.requestId == resolvedId
                        && (resolvedThreadId == nil || request.threadId == nil || request.threadId == resolvedThreadId)
                }
                pendingUserInputRequests.removeAll { request in
                    request.requestId == resolvedId
                        && (resolvedThreadId == nil || request.threadId == nil || request.threadId == resolvedThreadId)
                }
                pendingMcpElicitationRequests.removeAll { request in
                    request.requestId == resolvedId
                        && (resolvedThreadId == nil || request.threadId == nil || request.threadId == resolvedThreadId)
                }
            } else if let resolvedThreadId {
                pendingServerRequests.removeAll { $0.threadId == resolvedThreadId }
                pendingUserInputRequests.removeAll { $0.threadId == resolvedThreadId }
                pendingMcpElicitationRequests.removeAll { $0.threadId == resolvedThreadId }
                respondingServerRequestKeys.removeAll()
            } else {
                clearPendingServerInteractions()
            }
        default:
            break
        }
    }

    private func notificationBelongsToActiveThread(_ params: JSONValue) -> Bool {
        notificationIsForActiveThread(params)
    }

    private func applyThreadSettingsNotification(_ params: JSONValue) {
        guard notificationBelongsToActiveThread(params) else { return }
        let settings = params["threadSettings"] ?? params["settings"] ?? .null
        if let nextModel = settings["model"]?.string {
            model = nextModel
        }
        if let nextProvider = settings["modelProvider"]?.string ?? settings["model_provider"]?.string {
            provider = nextProvider
        }
        if let nextEffort = settings["effort"]?.string {
            effort = Self.displayEffort(nextEffort)
        }
        if let activePermission = settings["activePermissionProfile"]?["id"]?.string
            ?? settings["active_permission_profile"]?["id"]?.string {
            permissionProfileId = activePermission
            fullAccess = activePermission == ":danger-full-access"
        }
        if let authProfile = settings["authProfile"]?.string ?? settings["auth_profile"]?.string {
            for index in authProfiles.indices {
                authProfiles[index].active = authProfiles[index].name == authProfile
            }
        }
    }

    private func upsertActiveGoalPlan(_ plan: GoalPlanInfo) {
        guard plan.threadId.isEmpty || plan.threadId == activeThreadId else { return }
        if let index = activeGoalPlans.firstIndex(where: { $0.planId == plan.planId }) {
            activeGoalPlans[index] = plan
        } else {
            activeGoalPlans.append(plan)
        }
    }

    private func notificationBelongsToActiveTurn(_ params: JSONValue) -> Bool {
        guard let threadId = Self.notificationThreadId(params) else { return true }
        return threadId == activeThreadId || threadId == activeTurnThreadId
    }

    private func notificationIsForActiveThread(_ params: JSONValue) -> Bool {
        guard let threadId = Self.notificationThreadId(params) else { return true }
        return threadId == activeThreadId
    }

    private static func notificationThreadId(_ params: JSONValue) -> String? {
        params["threadId"]?.string
            ?? params["thread"]?["id"]?.string
            ?? params["item"]?["threadId"]?.string
            ?? params["item"]?["thread"]?["id"]?.string
    }

    func handleServerRequest(_ request: AppServerClient.ServerRequest) {
        let requestThreadId = Self.notificationThreadId(request.params)

        switch request.method {
        case "item/commandExecution/requestApproval":
            let command = request.params["command"]?.string ?? "Run command"
            let reason = request.params["reason"]?.string
            let cwd = request.params["cwd"]?.string
            let detail = [command, reason, cwd].compactMap { value in
                value?.isEmpty == false ? value : nil
            }.joined(separator: "\n")
            pendingServerRequests.append(PendingServerRequest(
                requestId: request.id,
                threadId: requestThreadId,
                method: request.method,
                kind: .commandApproval,
                title: "Approve command?",
                detail: detail.isEmpty ? command : detail,
                actions: Self.commandApprovalActions(from: request.params)))
        case "item/fileChange/requestApproval":
            let reason = request.params["reason"]?.string
            let root = request.params["grantRoot"]?.string
            let detail = [reason, root].compactMap { value in
                value?.isEmpty == false ? value : nil
            }.joined(separator: "\n")
            pendingServerRequests.append(PendingServerRequest(
                requestId: request.id,
                threadId: requestThreadId,
                method: request.method,
                kind: .fileChangeApproval,
                title: "Approve file changes?",
                detail: detail.isEmpty ? "The agent wants to edit files." : detail,
                actions: Self.fileChangeApprovalActions(grantRoot: root)))
        case "item/permissions/requestApproval":
            let permissions = request.params["permissions"] ?? .object([:])
            let reason = request.params["reason"]?.string
            let cwd = request.params["cwd"]?.string
            let summary = Self.permissionsSummary(permissions)
            let detail = [reason, cwd, summary].compactMap { value in
                value?.isEmpty == false ? value : nil
            }.joined(separator: "\n")
            pendingServerRequests.append(PendingServerRequest(
                requestId: request.id,
                threadId: requestThreadId,
                method: request.method,
                kind: .permissionsApproval,
                title: "Approve permissions?",
                detail: detail.isEmpty ? "The agent wants additional permissions." : detail,
                requestedPermissions: permissions,
                actions: Self.permissionsApprovalActions(permissions)))
        case "item/tool/requestUserInput":
            let questions = Self.userInputQuestions(from: request.params)
            guard !questions.isEmpty else {
                client.respondError(to: request.id, message: "request_user_input request did not include questions.")
                return
            }
            let title = questions.count == 1
                ? "Input requested"
                : "Input requested (\(questions.count) questions)"
            pendingUserInputRequests.append(PendingUserInputRequest(
                requestId: request.id,
                threadId: requestThreadId,
                title: title,
                questions: questions))
        case "mcpServer/elicitation/request":
            if let elicitation = Self.mcpElicitationRequest(from: request.params, requestId: request.id) {
                pendingMcpElicitationRequests.append(elicitation)
            } else {
                client.respond(to: request.id, result: Self.mcpElicitationResponse(action: "decline"))
            }
        case "account/chatgptAuthTokens/refresh":
            let message = Self.chatgptAuthTokensRefreshUnsupportedMessage(params: request.params)
            loginInProgress = false
            loginError = message
            client.respondError(to: request.id, code: -32000, message: message)
        case "item/tool/call":
            client.respond(to: request.id, result: Self.dynamicToolCallUnsupportedResponse(params: request.params))
        case "attestation/generate":
            client.respondError(
                to: request.id,
                code: -32000,
                message: "CodeWith.app did not advertise requestAttestation; attestation generation is unavailable.")
        default:
            if requestThreadId == nil || requestThreadId == activeThreadId {
                activeMessages.append(ChatMessage(role: .tool,
                    text: "Unsupported app-server request: \(request.method)",
                    toolIcon: "exclamationmark.triangle"))
            }
            client.respondError(to: request.id, message: "CodeWith.app does not support \(request.method) yet.")
        }
    }

    var pendingServerRequestForActiveThread: PendingServerRequest? {
        pendingServerRequests.first { request in
            request.threadId == nil || request.threadId == activeThreadId
        }
    }

    var pendingMcpElicitationForActiveThread: PendingMcpElicitationRequest? {
        pendingMcpElicitationRequests.first { request in
            request.threadId == nil || request.threadId == activeThreadId
        }
    }

    func respondToMcpElicitationRequest(
        _ prompt: PendingMcpElicitationRequest,
        action: String,
        content: JSONValue? = nil
    ) {
        guard beginServerRequestResponse(to: prompt.requestId) else { return }
        pendingMcpElicitationRequests.removeAll { $0.id == prompt.id }
        client.respond(to: prompt.requestId, result: Self.mcpElicitationResponse(action: action, content: content))
    }

    func openMcpElicitationURL(_ prompt: PendingMcpElicitationRequest) {
        guard case .url(let urlString) = prompt.mode, let url = URL(string: urlString) else { return }
        _ = NSWorkspace.shared.open(url)
    }

    static func mcpElicitationResponse(action: String, content: JSONValue? = nil) -> JSONValue {
        .object([
            "action": .string(action),
            "content": content ?? .null,
            "_meta": .null,
        ])
    }

    static func chatgptAuthTokensRefreshUnsupportedMessage(params: JSONValue) -> String {
        let accountHint = params["previousAccountId"]?.string
            .map { " for account \($0)" } ?? ""
        return "CodeWith.app cannot provide external ChatGPT auth tokens\(accountHint). It uses app-server managed login; sign in again if authentication expired."
    }

    static func dynamicToolCallUnsupportedResponse(params: JSONValue) -> JSONValue {
        let toolName = [params["namespace"]?.string, params["tool"]?.string]
            .compactMap { $0 }
            .filter { !$0.isEmpty }
            .joined(separator: "/")
        let displayName = toolName.isEmpty ? "dynamic tool" : toolName
        return .object([
            "contentItems": .array([
                .object([
                    "type": .string("inputText"),
                    "text": .string("CodeWith.app did not register \(displayName), so it cannot run this dynamic tool call."),
                ]),
            ]),
            "success": .bool(false),
        ])
    }

    var pendingUserInputForActiveThread: PendingUserInputRequest? {
        pendingUserInputRequests.first { request in
            request.threadId == nil || request.threadId == activeThreadId
        }
    }

    func respondToUserInputRequest(_ prompt: PendingUserInputRequest, answers: [String: [String]]) {
        guard beginServerRequestResponse(to: prompt.requestId) else { return }
        pendingUserInputRequests.removeAll { $0.id == prompt.id }
        client.respond(to: prompt.requestId, result: Self.userInputResponse(for: prompt, answers: answers))
    }

    func cancelUserInputRequest(_ prompt: PendingUserInputRequest) {
        respondToUserInputRequest(prompt, answers: [:])
    }

    static func userInputResponse(
        for prompt: PendingUserInputRequest,
        answers: [String: [String]]
    ) -> JSONValue {
        var mapped: [String: JSONValue] = [:]
        for question in prompt.questions {
            let values = answers[question.id] ?? []
            mapped[question.id] = .object([
                "answers": .array(values.map(JSONValue.string)),
            ])
        }
        return .object(["answers": .object(mapped)])
    }

    static func mcpElicitationContent(
        for prompt: PendingMcpElicitationRequest,
        values: [String: JSONValue]
    ) -> JSONValue {
        guard !prompt.fields.isEmpty else { return .null }
        var content: [String: JSONValue] = [:]
        for field in prompt.fields {
            if let value = values[field.id], !value.isNull {
                content[field.id] = value
            } else if let defaultValue = field.defaultValue {
                content[field.id] = defaultValue
            }
        }
        return .object(content)
    }

    func respondToServerRequest(_ prompt: PendingServerRequest, action: PendingServerRequestAction) {
        guard beginServerRequestResponse(to: prompt.requestId) else { return }
        pendingServerRequests.removeAll { $0.id == prompt.id }
        client.respond(to: prompt.requestId, result: action.result)
    }

    func respondToServerRequest(_ prompt: PendingServerRequest, approve: Bool) {
        if let action = prompt.actions.first(where: { action in
            action.key == (approve ? "accept" : "decline")
                || action.key == (approve ? "permissions-accept" : "permissions-decline")
        }) {
            respondToServerRequest(prompt, action: action)
            return
        }

        guard let fallback = Self.approvalAction(
            decision: .string(approve ? "accept" : "decline"),
            key: approve ? "accept" : "decline"
        ) else {
            return
        }
        respondToServerRequest(prompt, action: fallback)
    }

    private static func commandApprovalActions(from params: JSONValue) -> [PendingServerRequestAction] {
        let actions = approvalActions(from: params["availableDecisions"]?.array)
        if !actions.isEmpty {
            return actions
        }

        if params["networkApprovalContext"] != nil {
            var decisions: [JSONValue] = [.string("cancel"), .string("accept"), .string("acceptForSession")]
            if let allow = params["proposedNetworkPolicyAmendments"]?.array?.first(where: { amendment in
                amendment["action"]?.string == "allow"
            }) {
                decisions.append(.object([
                    "applyNetworkPolicyAmendment": .object([
                        "network_policy_amendment": allow,
                    ]),
                ]))
            }
            return approvalActions(from: decisions)
        }

        if params["additionalPermissions"] != nil {
            return approvalActions(from: [.string("cancel"), .string("accept")])
        }

        var decisions: [JSONValue] = [.string("cancel"), .string("accept")]
        if let amendment = params["proposedExecpolicyAmendment"] {
            decisions.append(.object([
                "acceptWithExecpolicyAmendment": .object([
                    "execpolicy_amendment": amendment,
                ]),
            ]))
        }
        return approvalActions(from: decisions)
    }

    private static func fileChangeApprovalActions(grantRoot: String?) -> [PendingServerRequestAction] {
        var decisions: [JSONValue] = [.string("decline"), .string("accept")]
        if grantRoot?.isEmpty == false {
            decisions.append(.string("acceptForSession"))
        }
        return approvalActions(from: decisions)
    }

    private static func permissionsApprovalActions(_ permissions: JSONValue) -> [PendingServerRequestAction] {
        [
            PendingServerRequestAction(
                key: "permissions-decline",
                title: "Decline",
                result: .object([
                    "scope": .string("turn"),
                    "permissions": .object([:]),
                ])),
            PendingServerRequestAction(
                key: "permissions-accept",
                title: "Approve",
                result: .object([
                    "scope": .string("turn"),
                    "permissions": permissions,
                ]),
                isPrimary: true),
        ]
    }

    private static func approvalActions(from decisions: [JSONValue]?) -> [PendingServerRequestAction] {
        var seen: [String: Int] = [:]
        return (decisions ?? []).compactMap { decision in
            guard var action = approvalAction(decision: decision, key: approvalActionKey(for: decision)) else {
                return nil
            }
            action.key = uniqueActionKey(action.key, seen: &seen)
            return action
        }
    }

    private static func approvalAction(decision: JSONValue, key: String) -> PendingServerRequestAction? {
        let result = JSONValue.object(["decision": decision])
        if let value = decision.string {
            switch value {
            case "accept":
                return PendingServerRequestAction(key: key, title: "Approve", result: result, isPrimary: true)
            case "acceptForSession":
                return PendingServerRequestAction(key: key, title: "Approve session", result: result, isPrimary: true)
            case "decline":
                return PendingServerRequestAction(key: key, title: "Decline", result: result)
            case "cancel":
                return PendingServerRequestAction(key: key, title: "Cancel", result: result)
            default:
                return nil
            }
        }

        guard let object = decision.object else { return nil }
        if let amendment = object["acceptWithExecpolicyAmendment"], !amendment.isNull {
            return PendingServerRequestAction(key: key, title: "Trust command", result: result, isPrimary: true)
        }
        if let amendment = object["applyNetworkPolicyAmendment"]?["network_policy_amendment"]
            ?? object["applyNetworkPolicyAmendment"]?["networkPolicyAmendment"],
            !amendment.isNull
        {
            let title: String
            let isPrimary: Bool
            switch amendment["action"]?.string {
            case "allow":
                title = "Allow host"
                isPrimary = true
            case "deny":
                title = "Block host"
                isPrimary = false
            default:
                title = "Apply network rule"
                isPrimary = true
            }
            return PendingServerRequestAction(key: key, title: title, result: result, isPrimary: isPrimary)
        }
        return nil
    }

    private static func approvalActionKey(for decision: JSONValue) -> String {
        if let value = decision.string { return value }
        if let object = decision.object?.keys.sorted().first { return object }
        return "decision"
    }

    private static func uniqueActionKey(_ key: String, seen: inout [String: Int]) -> String {
        let count = seen[key] ?? 0
        seen[key] = count + 1
        return count == 0 ? key : "\(key)-\(count + 1)"
    }

    private static func userInputQuestions(from params: JSONValue) -> [PendingUserInputQuestion] {
        (params["questions"]?.array ?? []).compactMap { value in
            guard let id = value["id"]?.string, !id.isEmpty else { return nil }
            let options = (value["options"]?.array ?? []).compactMap { option -> PendingUserInputOption? in
                guard let label = option["label"]?.string, !label.isEmpty else { return nil }
                return PendingUserInputOption(
                    label: label,
                    description: option["description"]?.string ?? "")
            }
            return PendingUserInputQuestion(
                id: id,
                header: value["header"]?.string ?? "",
                question: value["question"]?.string ?? "",
                isOther: value["isOther"]?.bool ?? false,
                isSecret: value["isSecret"]?.bool ?? false,
                options: options)
        }
    }

    private static func mcpElicitationRequest(
        from params: JSONValue,
        requestId: JSONValue
    ) -> PendingMcpElicitationRequest? {
        let threadId = Self.notificationThreadId(params)
        let serverName = params["serverName"]?.string ?? "MCP server"
        let message = params["message"]?.string ?? "The MCP server needs input."
        switch params["mode"]?.string {
        case "form":
            let requestedSchema = params["requestedSchema"]
            let fields: [PendingMcpElicitationField]
            if requestedSchema == nil || requestedSchema?.isNull == true {
                fields = []
            } else {
                guard let parsedFields = mcpElicitationFields(from: requestedSchema) else { return nil }
                fields = parsedFields
            }
            return PendingMcpElicitationRequest(
                requestId: requestId,
                threadId: threadId,
                serverName: serverName,
                title: "MCP input requested",
                message: message,
                mode: .form,
                fields: fields)
        case "url":
            guard let url = validMcpElicitationURL(params["url"]?.string) else { return nil }
            return PendingMcpElicitationRequest(
                requestId: requestId,
                threadId: threadId,
                serverName: serverName,
                title: "Action required",
                message: message,
                mode: .url(url),
                fields: [])
        default:
            return nil
        }
    }

    private static func mcpElicitationFields(from schema: JSONValue?) -> [PendingMcpElicitationField]? {
        guard schema?["type"]?.string == "object" else { return nil }
        guard let properties = schema?["properties"]?.object else { return nil }
        let required = Set(schema?["required"]?.array?.compactMap(\.string) ?? [])
        var fields: [PendingMcpElicitationField] = []
        for id in properties.keys.sorted() {
            guard let property = properties[id] else { return nil }
            guard let field = mcpElicitationField(id: id, property: property, required: required.contains(id)) else {
                return nil
            }
            fields.append(field)
        }
        return fields
    }

    private static func mcpElicitationField(
        id: String,
        property: JSONValue,
        required: Bool
    ) -> PendingMcpElicitationField? {
        let label = property["title"]?.string ?? id
        let prompt = property["description"]?.string ?? label
        switch property["type"]?.string {
        case "string":
            if let options = mcpElicitationOptions(from: property, defaultValues: [property["default"]].compactMap { $0 }),
               !options.isEmpty {
                return PendingMcpElicitationField(
                    id: id,
                    label: label,
                    prompt: prompt,
                    required: required,
                    kind: .singleSelect,
                    options: options,
                    defaultValue: property["default"])
            }
            let kind: PendingMcpElicitationField.Kind = property["format"]?.string == "password" ? .secret : .text
            return PendingMcpElicitationField(
                id: id,
                label: label,
                prompt: prompt,
                required: required,
                kind: kind,
                defaultValue: property["default"])
        case "boolean":
            let options = [
                PendingMcpElicitationOption(
                    label: "True",
                    value: .bool(true),
                    isDefault: property["default"] == .bool(true)),
                PendingMcpElicitationOption(
                    label: "False",
                    value: .bool(false),
                    isDefault: property["default"] == .bool(false)),
            ]
            return PendingMcpElicitationField(
                id: id,
                label: label,
                prompt: prompt,
                required: required,
                kind: .singleSelect,
                options: options,
                defaultValue: property["default"])
        case "number":
            return PendingMcpElicitationField(
                id: id,
                label: label,
                prompt: prompt,
                required: required,
                kind: .number,
                defaultValue: property["default"])
        case "integer":
            return PendingMcpElicitationField(
                id: id,
                label: label,
                prompt: prompt,
                required: required,
                kind: .integer,
                defaultValue: property["default"])
        case "array":
            let defaultValues = property["default"]?.array ?? []
            guard let options = mcpElicitationOptions(from: property["items"], defaultValues: defaultValues),
                  !options.isEmpty else {
                return nil
            }
            return PendingMcpElicitationField(
                id: id,
                label: label,
                prompt: prompt,
                required: required,
                kind: .multiSelect,
                options: options,
                defaultValue: property["default"])
        default:
            return nil
        }
    }

    private static func mcpElicitationOptions(
        from schema: JSONValue?,
        defaultValues: [JSONValue]
    ) -> [PendingMcpElicitationOption]? {
        if let entries = schema?["oneOf"]?.array ?? schema?["anyOf"]?.array {
            let options = entries.compactMap { entry -> PendingMcpElicitationOption? in
                guard let value = entry["const"] else { return nil }
                return PendingMcpElicitationOption(
                    label: entry["title"]?.string ?? displayString(for: value),
                    value: value,
                    isDefault: defaultValues.contains(value))
            }
            return options.isEmpty ? nil : options
        }

        guard let values = schema?["enum"]?.array else { return nil }
        let names = schema?["enumNames"]?.array?.compactMap(\.string) ?? []
        return values.enumerated().map { index, value in
            PendingMcpElicitationOption(
                label: index < names.count ? names[index] : displayString(for: value),
                value: value,
                isDefault: defaultValues.contains(value))
        }
    }

    private static func validMcpElicitationURL(_ value: String?) -> String? {
        guard let value, let url = URL(string: value),
              url.scheme == "https",
              url.host != nil,
              url.user == nil,
              url.password == nil else {
            return nil
        }
        return url.absoluteString
    }

    private static func displayString(for value: JSONValue) -> String {
        switch value {
        case .string(let string):
            return string
        case .bool(let bool):
            return bool ? "true" : "false"
        case .number(let number):
            return number.truncatingRemainder(dividingBy: 1) == 0
                ? String(Int(number))
                : String(number)
        case .null:
            return "null"
        case .array, .object:
            return "value"
        }
    }

    private static func permissionsSummary(_ permissions: JSONValue) -> String {
        var parts: [String] = []
        if permissions["network"] != nil { parts.append("Network access") }
        if let fileSystem = permissions["fileSystem"] {
            if let read = fileSystem["read"]?.array?.compactMap(\.string), !read.isEmpty {
                parts.append("Read: \(read.joined(separator: ", "))")
            }
            if let write = fileSystem["write"]?.array?.compactMap(\.string), !write.isEmpty {
                parts.append("Write: \(write.joined(separator: ", "))")
            }
        }
        return parts.joined(separator: "\n")
    }

    private func handleCompletedItem(_ item: JSONValue) {
        guard !item.isNull else { return }
        let type = item["type"]?.string
        // The server echoes the user's own message back as a completed item; we
        // already added it optimistically on submit — skip to avoid a duplicate.
        if type == "userMessage" { return }
        if type == "agentMessage" {
            // If nothing streamed via deltas, append the finalized text.
            if streamingAssistantIndex == nil, let msg = AppServerClient.parseItem(item) {
                activeMessages.append(msg)
            }
            streamingAssistantIndex = nil   // finalize; next message starts fresh
        } else if type == "commandExecution", updateStreamedCommandExecution(item) {
            streamingAssistantIndex = nil
        } else if let msg = AppServerClient.parseItem(item) {
            activeMessages.append(msg)
            streamingAssistantIndex = nil   // a tool/other item ends the current assistant bubble
        }
    }

    private func appendAssistantDelta(_ delta: String) {
        guard !delta.isEmpty else { return }
        if let i = streamingAssistantIndex, i < activeMessages.count {
            activeMessages[i].text += delta
        } else {
            activeMessages.append(ChatMessage(role: .assistant, text: delta))
            streamingAssistantIndex = activeMessages.count - 1
        }
    }

    private func appendToolOutputDelta(method: String, params: JSONValue) {
        guard let delta = Self.toolOutputDeltaText(method: method, params: params), !delta.isEmpty else {
            return
        }
        let key = Self.toolOutputKey(method: method, params: params) ?? "\(method):\(activeMessages.count)"
        let stream = params["stream"]?.string
        appendToolOutput(key: key, title: Self.toolOutputTitle(method: method, stream: stream), delta: delta)
    }

    private func appendTerminalInteraction(_ params: JSONValue) {
        let stdin = params["stdin"]?.string ?? ""
        let detail = stdin.isEmpty ? "Terminal input requested." : "Terminal input requested:\n\(stdin)"
        let key = Self.toolOutputKey(method: "item/commandExecution/outputDelta", params: params)
            ?? "terminalInteraction:\(activeMessages.count)"
        appendToolOutput(key: key, title: "Terminal interaction", delta: detail)
    }

    private func appendToolOutput(key: String, title: String, delta: String) {
        if let index = toolOutputMessageIndexes[key], activeMessages.indices.contains(index) {
            activeMessages[index].text += delta
            return
        }
        let message = ChatMessage(role: .tool, text: "\(title):\n\(delta)", toolIcon: "terminal")
        activeMessages.append(message)
        toolOutputMessageIndexes[key] = activeMessages.count - 1
        streamingAssistantIndex = nil
    }

    private func updateStreamedCommandExecution(_ item: JSONValue) -> Bool {
        guard let itemId = item["id"]?.string else { return false }
        let key = Self.commandExecutionOutputKey(itemId)
        guard let index = toolOutputMessageIndexes[key], activeMessages.indices.contains(index) else {
            return false
        }
        let command = item["command"]?.string ?? "command"
        if !activeMessages[index].text.hasPrefix("Ran ") {
            activeMessages[index].text = "Ran \(command)\n\(activeMessages[index].text)"
        }
        if let status = item["status"]?.string, status != "completed",
           !activeMessages[index].text.contains("Status: \(status)") {
            activeMessages[index].text += "\nStatus: \(status)"
        }
        toolOutputMessageIndexes.removeValue(forKey: key)
        return true
    }

    private static func toolOutputKey(method: String, params: JSONValue) -> String? {
        switch method {
        case "item/commandExecution/outputDelta":
            return params["itemId"]?.string.map(commandExecutionOutputKey)
        case "command/exec/outputDelta":
            return params["processId"]?.string.map { "commandExec:\($0)" }
        case "process/outputDelta":
            return params["processHandle"]?.string.map { "process:\($0)" }
        default:
            return nil
        }
    }

    private static func commandExecutionOutputKey(_ itemId: String) -> String {
        "commandExecution:\(itemId)"
    }

    private static func toolOutputTitle(method: String, stream: String?) -> String {
        let streamSuffix = stream == "stderr" ? " stderr" : ""
        switch method {
        case "process/outputDelta":
            return "Process\(streamSuffix) output"
        default:
            return "Command\(streamSuffix) output"
        }
    }

    private static func toolOutputDeltaText(method: String, params: JSONValue) -> String? {
        if method == "item/commandExecution/outputDelta" {
            return params["delta"]?.string
        }
        guard let encoded = params["deltaBase64"]?.string,
              let data = Data(base64Encoded: encoded) else {
            return nil
        }
        var text = String(decoding: data, as: UTF8.self)
        if params["capReached"]?.bool == true {
            text += "\n[output truncated]"
        }
        return text
    }

    // MARK: Config actions

    func setModel(_ m: String) {
        let previousModel = model
        model = m
        if let activeThreadId {
            Task {
                let ok = await writeThreadSettings(threadId: activeThreadId, model: m)
                if !ok {
                    model = previousModel
                }
            }
        } else {
            Task { await writeConfigValue(keyPath: "model", value: .string(m), reloadUserConfig: true) }
        }
    }
    func setProvider(_ p: String) {
        let previousProvider = provider
        provider = p
        if let activeThreadId {
            Task {
                if await writeThreadSettings(threadId: activeThreadId, provider: p) {
                    await loadModelCatalog()
                } else {
                    provider = previousProvider
                }
            }
        } else {
            Task {
                await writeConfigValue(keyPath: "model_provider", value: .string(p), reloadUserConfig: true)
                if configError == nil { await loadModelCatalog() }
            }
        }
    }
    func setEffort(_ e: String) {
        let previousEffort = effort
        effort = e
        let wireEffort = Self.wireEffort(e)
        if let activeThreadId {
            Task {
                let ok = await writeThreadSettings(threadId: activeThreadId, effort: wireEffort)
                if !ok {
                    effort = previousEffort
                }
            }
        } else {
            Task { await writeConfigValue(keyPath: "model_reasoning_effort", value: .string(wireEffort), reloadUserConfig: true) }
        }
    }
    func setApproval(_ a: String) {
        guard approvalOptions.contains(a) else {
            configError = "Approval policy \(a) is blocked by managed requirements."
            return
        }
        configApproval = a
        Task { await writeConfigValue(keyPath: "approval_policy", value: .string(a), reloadUserConfig: true) }
    }
    func setSandbox(_ s: String) {
        guard sandboxOptions.contains(s) else {
            configError = "Sandbox mode \(s) is blocked by managed requirements."
            return
        }
        configSandbox = s
        permissionProfileId = Self.permissionProfileId(forSandbox: s)
        fullAccess = s == "danger-full-access"
        if s != "danger-full-access" { previousNonFullSandbox = s }
        Task { await writeConfigValue(keyPath: "sandbox_mode", value: .string(s), reloadUserConfig: true) }
    }
    func setFullAccess(_ on: Bool) {
        if activeThreadId != nil {
            setPermissionProfile(on ? ":danger-full-access" : ":workspace")
            return
        }
        guard !on || canUseFullAccess else {
            configError = "Full access is blocked by managed requirements."
            return
        }
        fullAccess = on
        if on, let sandbox = configSandbox, sandbox != "danger-full-access" {
            previousNonFullSandbox = sandbox
        }
        setSandbox(on ? "danger-full-access" : previousNonFullSandbox)
    }
    func setPermissionProfile(_ profileId: String) {
        let previousProfileId = permissionProfileId
        let previousFullAccess = fullAccess
        permissionProfileId = profileId
        fullAccess = profileId == ":danger-full-access"
        if let activeThreadId {
            Task {
                let ok = await writeThreadSettings(threadId: activeThreadId, permissions: profileId)
                if !ok {
                    permissionProfileId = previousProfileId
                    fullAccess = previousFullAccess
                }
            }
            return
        }

        switch profileId {
        case ":danger-full-access":
            setFullAccess(true)
        case ":workspace":
            setSandbox("workspace-write")
        case ":read-only":
            setSandbox("read-only")
        default:
            Task { await writeConfigValue(keyPath: "default_permissions", value: .string(profileId), reloadUserConfig: true) }
        }
    }
    func setSessionAuthProfile(_ name: String) {
        if let activeThreadId {
            Task {
                let ok = await writeThreadSettings(threadId: activeThreadId, authProfile: name)
                if ok {
                    for index in authProfiles.indices {
                        authProfiles[index].active = authProfiles[index].name == name
                    }
                }
            }
        } else {
            Task { await switchAuthProfile(name) }
        }
    }
    func setWorkMode(_ value: String) {
        desktopSettings.workMode = value
        Task { await writeConfigValue(keyPath: "desktop.workMode", value: .string(value)) }
    }
    func setFileOpenDestination(_ value: String) {
        desktopSettings.fileOpenDestination = value
        Task {
            await writeConfigValues([
                (keyPath: "desktop.fileOpenDestination", value: .string(value)),
                (keyPath: "file_opener", value: .string(Self.fileOpenerConfigValue(for: value))),
            ], reloadUserConfig: false)
        }
    }
    func setLanguage(_ value: String) {
        desktopSettings.language = value
        Task { await writeConfigValue(keyPath: "desktop.language", value: .string(value)) }
    }
    func setShowMenuBar(_ on: Bool) {
        desktopSettings.showMenuBar = on
        publishMenuBarPreference(on)
        Task { await writeConfigValue(keyPath: "desktop.showMenuBar", value: .bool(on)) }
    }
    func setBottomPanel(_ on: Bool) {
        desktopSettings.bottomPanel = on
        Task { await writeConfigValue(keyPath: "desktop.bottomPanel", value: .bool(on)) }
    }
    func setPersonality(_ value: String) {
        desktopSettings.personality = value
        Task {
            await writeConfigValue(keyPath: "personality", value: .string(value))
            if let activeThreadId {
                do {
                    try await client.updateThreadPersonality(threadId: activeThreadId, personality: value)
                } catch {
                    configError = error.localizedDescription
                }
            }
        }
    }
    func setCustomInstructions(_ value: String) {
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        customInstructions = value
        Task {
            await writeConfigValue(
                keyPath: "developer_instructions",
                value: trimmed.isEmpty ? .null : .string(value),
                reloadUserConfig: true)
        }
    }
    func setMemoryEnabled(_ on: Bool) {
        desktopSettings.memoryEnabled = on
        Task { await writeMemoryEnabled(on) }
    }
    func setChronicleResearch(_ on: Bool) {
        desktopSettings.chronicleResearch = on
        Task { await writeConfigValue(keyPath: "features.chronicle", value: .bool(on), reloadUserConfig: true) }
    }
    func setSkipToolAssistedChats(_ on: Bool) {
        desktopSettings.skipToolAssistedChats = on
        Task { await writeConfigValue(keyPath: "memories.disable_on_external_context", value: .bool(on), reloadUserConfig: true) }
    }
    func resetMemories() {
        Task { await resetMemoriesWithServer() }
    }

    private func writeMemoryEnabled(_ on: Bool) async {
        configError = nil
        do {
            _ = try await client.batchWriteConfig(
                edits: [
                    (keyPath: "features.memories", value: .bool(on)),
                    (keyPath: "memories.use_memories", value: .bool(on)),
                    (keyPath: "memories.generate_memories", value: .bool(on)),
                ],
                reloadUserConfig: true)
            if let activeThreadId {
                try await client.setThreadMemoryMode(threadId: activeThreadId, enabled: on)
            }
        } catch {
            configError = error.localizedDescription
        }
        await loadConfig()
    }

    private func resetMemoriesWithServer() async {
        configError = nil
        do {
            try await client.resetMemories()
        } catch {
            configError = error.localizedDescription
        }
    }

    private func publishMenuBarPreference(_ enabled: Bool) {
        guard publishedMenuBarPreference != enabled else { return }
        publishedMenuBarPreference = enabled
        NotificationCenter.default.post(name: .codeWithMenuBarPreferenceChanged, object: enabled)
    }

    private func writeConfigValue(keyPath: String, value: JSONValue, reloadUserConfig: Bool = false) async {
        await writeConfigValues([(keyPath: keyPath, value: value)], reloadUserConfig: reloadUserConfig)
    }

    private func writeThreadSettings(
        threadId: String,
        model: String? = nil,
        provider: String? = nil,
        effort: String? = nil,
        permissions: String? = nil,
        authProfile: String? = nil
    ) async -> Bool {
        configError = nil
        do {
            try await client.updateThreadSettings(
                threadId: threadId,
                model: model,
                provider: provider,
                effort: effort,
                permissions: permissions,
                authProfile: authProfile)
            return true
        } catch {
            configError = error.localizedDescription
            return false
        }
    }

    private func writeConfigValues(_ edits: [(keyPath: String, value: JSONValue)], reloadUserConfig: Bool) async {
        configError = nil
        do {
            if reloadUserConfig || edits.count > 1 {
                _ = try await client.batchWriteConfig(edits: edits, reloadUserConfig: reloadUserConfig)
            } else if let edit = edits.first {
                _ = try await client.writeConfig(keyPath: edit.keyPath, value: edit.value)
            }
        } catch {
            configError = error.localizedDescription
        }
        await loadConfig()
    }

    func openConfigToml() {
        #if canImport(AppKit)
        let env = ProcessInfo.processInfo.environment
        let path = Self.configTomlPath(
            serverCodewithHome: serverCodewithHome,
            environmentCodewithHome: env["CODEWITH_HOME"],
            homeDirectory: NSHomeDirectory())
        let url = URL(fileURLWithPath: path)
        openLocalFile(url)
        #endif
    }

    func openDiagnosticsLog() {
        #if canImport(AppKit)
        openLocalFile(URL(fileURLWithPath: "/tmp/codewith-diag.log"))
        #endif
    }

    private static func fileOpenerConfigValue(for destination: String) -> String {
        destination == "cursor" ? "cursor" : "none"
    }

    static func configTomlPath(
        serverCodewithHome: String?,
        environmentCodewithHome: String?,
        homeDirectory: String
    ) -> String {
        let codewithHome = serverCodewithHome.flatMap { $0.isEmpty ? nil : $0 }
            ?? environmentCodewithHome.flatMap { $0.isEmpty ? nil : $0 }
            ?? "\(homeDirectory)/.codewith"
        return URL(fileURLWithPath: codewithHome).appendingPathComponent("config.toml").path
    }

    private func openLocalFile(_ url: URL) {
        #if canImport(AppKit)
        switch desktopSettings.fileOpenDestination {
        case "finder":
            NSWorkspace.shared.activateFileViewerSelecting([url])
        case "cursor":
            if let cursorURL = cursorFileURL(for: url), NSWorkspace.shared.open(cursorURL) {
                return
            }
            NSWorkspace.shared.open(url)
        default:
            NSWorkspace.shared.open(url)
        }
        #endif
    }

    private func cursorFileURL(for url: URL) -> URL? {
        let path = url.standardizedFileURL.path
        guard let encodedPath = path.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) else {
            return nil
        }
        return URL(string: "cursor://file\(encodedPath)")
    }

    static func wireEffort(_ label: String) -> String {
        switch label.lowercased().replacingOccurrences(of: " ", with: "") {
        case "extrahigh", "xhigh": return "xhigh"
        case "medium": return "medium"
        case "high": return "high"
        case "minimal": return "minimal"
        case "none": return "none"
        default: return "low"
        }
    }

    static func displayEffort(_ wireValue: String) -> String {
        switch wireValue.lowercased() {
        case "xhigh", "extrahigh": return "Extra High"
        case "medium": return "Medium"
        case "high": return "High"
        case "minimal": return "Minimal"
        case "none": return "None"
        default: return "Low"
        }
    }

    static func displayProvider(_ provider: String) -> String {
        switch provider.lowercased() {
        case "openai": return "OpenAI"
        case "openrouter": return "OpenRouter"
        case "azure": return "Azure"
        case "anthropic": return "Anthropic"
        case "ollama": return "Ollama"
        default:
            return provider
                .split(separator: "-")
                .map { $0.prefix(1).uppercased() + String($0.dropFirst()) }
                .joined(separator: " ")
        }
    }

    static func permissionProfileId(forSandbox sandbox: String?) -> String {
        switch sandbox {
        case "danger-full-access": return ":danger-full-access"
        case "read-only": return ":read-only"
        default: return ":workspace"
        }
    }

    static func displayModel(_ model: String) -> String {
        model
            .replacingOccurrences(of: "gpt-", with: "GPT-")
            .replacingOccurrences(of: "codex", with: "Codex")
    }

    static func displayPermissionProfile(_ profileId: String) -> String {
        switch profileId {
        case ":danger-full-access": return "Full Access"
        case ":workspace": return "Workspace Write"
        case ":read-only": return "Read Only"
        default: return profileId
        }
    }

    // MARK: Add menu

    func toggleAddMenu() {
        showAddMenu.toggle()
        if showAddMenu {
            Task { await loadActivePeers(); await loadAgentRuns() }
        }
    }
    func handleAddAction(_ action: AddMenuAction) {
        switch action {
        case .planMode:
            planMode = true
            showAddMenu = false
        case .goal:
            prefixGoalComposer()
        case .filesAndFolders:
            pickFolder()
        case .attachGhostty:
            showAddMenu = false
        case .activePeer(let peerId):
            if let peer = activePeers.first(where: { $0.peerId == peerId }) {
                Task { await sendComposerToActivePeer(peer) }
                return
            }
            showAddMenu = false
        case .agentRun(let agentId):
            Task { await openAgentRun(agentId: agentId) }
        }
    }

    func sendComposerToActivePeer(_ peer: ActiveSessionPeerInfo) async {
        let message = composerText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !message.isEmpty else {
            pendingActivePeer = peer
            composerText = "@\(peer.displayName) "
            showAddMenu = false
            return
        }
        guard connection == .connected else {
            pendingActivePeer = peer
            composerText = "@\(peer.displayName) \(message)"
            showAddMenu = false
            return
        }
        pendingActivePeer = nil
        let senderThreadId = await ensureActiveThread()
        if let senderThreadId { route = .chat(senderThreadId) }
        do {
            let status = try await client.sendActiveSessionMessage(
                targetPeerId: peer.peerId,
                message: message,
                senderThreadId: senderThreadId,
                delivery: peer.canTriggerTurn ? "triggerTurn" : "queueOnly"
            )
            if status == "delivered" {
                activeMessages.append(ChatMessage(role: .tool,
                    text: "Sent to \(peer.displayName)",
                    toolIcon: "paperplane"))
                composerText = ""
            } else {
                activeMessages.append(ChatMessage(role: .tool,
                    text: "\(peer.displayName) is \(status).",
                    toolIcon: "exclamationmark.triangle"))
            }
        } catch {
            activeMessages.append(ChatMessage(role: .tool,
                text: "Could not send to \(peer.displayName): \(error.localizedDescription)",
                toolIcon: "exclamationmark.triangle"))
        }
        showAddMenu = false
    }

    static func activePeerMessage(from text: String, peer: ActiveSessionPeerInfo) -> String {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        let mention = "@\(peer.displayName)"
        if trimmed == mention { return "" }
        if trimmed.hasPrefix(mention) {
            return String(trimmed.dropFirst(mention.count)).trimmingCharacters(in: .whitespacesAndNewlines)
        }
        return trimmed
    }

    func openAgentRun(agentId: String) async {
        showAddMenu = false
        guard let agent = agentRuns.first(where: { $0.agentId == agentId }), agent.canOpenThread else { return }
        let attachment: AgentAttachmentInfo?
        let attachError: Error?
        do {
            let attached = try await client.attachAgentRun(agentId: agentId)
            if let updatedAgent = attached.agent {
                upsertAgentRun(updatedAgent)
            }
            attachment = attached
            attachError = nil
        } catch {
            attachment = try? await client.readAgentRun(agentId: agentId)
            attachError = error
        }
        let thread = threads.first { $0.id == agent.threadId }
            ?? ThreadInfo(from: .object([
                "id": .string(agent.threadId),
                "name": .string(agent.displayName),
                "cwd": agent.rolloutPath.isEmpty ? .null : .string(agent.rolloutPath),
            ]))
        await openThread(thread)
        activeAgentAttachment = attachment
        if let attachError {
            activeMessages.append(ChatMessage(
                role: .tool,
                text: "Agent attach failed: \(attachError.localizedDescription)",
                toolIcon: "exclamationmark.triangle"))
        }
        if let attachment, attachment.pendingCount > 0 {
            activeMessages.append(ChatMessage(
                role: .tool,
                text: "Agent has \(attachment.pendingCount) pending interaction\(attachment.pendingCount == 1 ? "" : "s").",
                toolIcon: "questionmark.bubble"))
        }
    }

    private func upsertAgentRun(_ agent: AgentRunInfo) {
        if let index = agentRuns.firstIndex(where: { $0.agentId == agent.agentId }) {
            agentRuns[index] = agent
        } else {
            agentRuns.append(agent)
        }
    }

    private func prefixGoalComposer() {
        if !composerText.hasPrefix("Goal: ") { composerText = "Goal: " + composerText }
        showAddMenu = false
    }

    static func goalObjective(from text: String) -> String {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        if Self.isGoalCommand(trimmed) {
            return String(trimmed.dropFirst(5)).trimmingCharacters(in: .whitespacesAndNewlines)
        }
        return trimmed
    }

    static func isGoalCommand(_ text: String) -> Bool {
        text.trimmingCharacters(in: .whitespacesAndNewlines).lowercased().hasPrefix("goal:")
    }

    static func loopPrompt(from text: String) -> String {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        if Self.isLoopCommand(trimmed) {
            return String(trimmed.dropFirst(5)).trimmingCharacters(in: .whitespacesAndNewlines)
        }
        return trimmed
    }

    static func isLoopCommand(_ text: String) -> Bool {
        text.trimmingCharacters(in: .whitespacesAndNewlines).lowercased().hasPrefix("loop:")
    }
    func pickFolder() {
        showAddMenu = false
        #if canImport(AppKit)
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true; panel.canChooseFiles = false
        panel.allowsMultipleSelection = false; panel.prompt = "Add Project"
        if panel.runModal() == .OK, let url = panel.url {
            projects.insert(ProjectInfo(name: url.lastPathComponent, path: url.path, groupKey: url.path,
                                        originUrl: nil, branch: nil, threadCount: 0, lastActivity: 0), at: 0)
            newSessionInProject(url.path)
        }
        #endif
    }

    func switchProfile(_ id: UUID) { currentProfileID = id }

    /// A disconnected model pre-populated with representative data, for
    /// snapshot rendering / previews (does not start the app-server).
    static func sample() -> AppModel {
        let m = AppModel()
        m.connection = .connected
        func t(_ id: String, _ name: String, _ cwd: String, _ age: String) -> ThreadInfo {
            ThreadInfo(from: .object([
                "id": .string(id), "name": .string(name), "cwd": .string(cwd),
                "updatedAt": .string(age),
            ]))
        }
        m.threads = [
            t("t1", "Say hi", "/Users/hasna/scaffold-api", "2026-06-22T13:58:00Z"),
            t("t2", "Add OAuth preparation", "/Users/hasna/scaffold-api", "2026-04-01T10:00:00Z"),
            t("t3", "Find and fix bug in codebase", "/Users/hasna/web-app", "2026-06-01T10:00:00Z"),
            t("t4", "Write granular e2e tests", "/Users/hasna/web-app", "2026-05-20T10:00:00Z"),
        ]
        m.nextCursor = "more"
        m.projects = ProjectInfo.derive(from: m.threads)
        m.loops = [
            LoopInfo(id: "l1", title: "Daily standup digest", subtitle: "every day · 9:00", kind: .schedule, active: true),
            LoopInfo(id: "l2", title: "PR babysitter", subtitle: "every 5m", kind: .monitor, active: true),
            LoopInfo(id: "l3", title: "Security sweep", subtitle: "weekly", kind: .schedule, active: false),
        ]
        m.apps = [
            AppItemInfo(name: "Mail", detail: "Read and send email from your agent.", enabled: true),
            AppItemInfo(name: "Deploy", detail: "Ship builds to staging and production.", enabled: false),
            AppItemInfo(name: "Image", detail: "Generate and edit images on demand.", enabled: true),
            AppItemInfo(name: "Deep Research", detail: "Fan-out, fact-checked reports.", enabled: false),
            AppItemInfo(name: "Search", detail: "Trigram local search.", enabled: true),
            AppItemInfo(name: "Memory", detail: "Persistent cross-session memory.", enabled: true),
        ]
        m.machines = [
            MachineInfo(id: "spark01", os: "linux", status: "online", role: "primary", isLocal: true),
            MachineInfo(id: "apple03", os: "macos", status: "online", role: "workstation", isLocal: false),
            MachineInfo(id: "machine001", os: "macos", status: "online", role: "build", isLocal: false),
            MachineInfo(id: "apple06", os: "macos", status: "unknown", role: "laptop", isLocal: false),
        ]
        m.authProfiles = [
            AuthProfileInfo(name: "account001", email: "theflashbadger@gmail.com", provider: "ChatGPT", plan: "Pro"),
            AuthProfileInfo(name: "account002", email: "andrei@hasna.com", provider: "ChatGPT", plan: "Pro"),
        ]
        m.account = AccountInfo(from: .object(["account": .object([
            "displayName": .string("Andrei Hasna"), "email": .string("andrei@hasna.com"), "planType": .string("Pro"),
        ])]))
        m.activeThreadId = "t1"
        m.activeMessages = [
            ChatMessage(role: .user, text: "hi"),
            ChatMessage(role: .assistant, text: "I'll register the session context first because the provided project rules make that mandatory before any real work."),
            ChatMessage(role: .tool, text: "Ran a command", toolIcon: "terminal"),
            ChatMessage(role: .assistant, text: "The first skill path was stale in this environment, so I'm using the installed CodeWith skill location and continuing with the required registration flow."),
            ChatMessage(role: .tool, text: "Read a file", toolIcon: "doc.text"),
        ]
        m.model = "gpt-5.5-codex"; m.provider = "openai"
        m.serverVersion = "0.137.0"; m.configApproval = "on-request"; m.configSandbox = "read-only"
        return m
    }
}
