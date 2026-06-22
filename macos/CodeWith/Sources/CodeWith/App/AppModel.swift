import SwiftUI

// MARK: - Routing

enum Route: Hashable {
    case home, search, apps, loops, machines, profiles
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
    var method: String
    var kind: Kind
    var title: String
    var detail: String
    var requestedPermissions: JSONValue? = nil
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
    var activeGoal: GoalInfo? = nil
    var apps: [AppItemInfo] = []
    var machines: [MachineInfo] = []
    var authProfiles: [AuthProfileInfo] = []
    var account = AccountInfo.signedOut
    var serverVersion: String? = nil
    var configApproval: String? = nil
    var configSandbox: String? = nil
    var remoteSearchThreads: [ThreadInfo] = []
    private var pendingOpenURL: URL? = nil

    // In-session config
    var model: String? = nil
    var provider: String? = nil
    var effort: String = "Low"
    var availableModels = ["gpt-5.5-codex", "gpt-5.5", "o3", "gpt-4.1"]
    var availableProviders = ["openai", "azure", "openrouter", "ollama"]
    let availableEfforts = ["Low", "Medium", "High", "Extra High"]

    // Active chat
    var activeThreadId: String? = nil
    var activeTurnId: String? = nil
    var activeMessages: [ChatMessage] = []
    var pendingServerRequests: [PendingServerRequest] = []
    var turnInProgress = false
    var currentProjectPath: String? = nil
    private var streamingAssistantIndex: Int? = nil

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
            }
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
        client.stop()
    }

    func reconnectAppServer() async {
        shutdown()
        turnInProgress = false
        activeTurnId = nil
        pendingServerRequests = []
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
        await loadConfig()
        async let catalog: () = loadModelCatalog()
        await loadThreads(reset: true)          // fast first-page paint
        await loadLoops()
        _ = await (acct, apps, catalog)
        // Drain remaining pages in the background so Projects becomes complete.
        Task { [weak self] in
            guard let self else { return }
            var guardCount = 0
            while await self.nextCursor != nil, guardCount < 60 {
                await self.loadThreads(reset: false)
                guardCount += 1
            }
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
        machines = (try? await client.listMachines()) ?? []
    }

    func toggleLoop(_ loop: LoopInfo) async {
        guard connection == .connected, !loop.threadId.isEmpty else { return }
        // Optimistic flip, then call backend and reload.
        if let i = loops.firstIndex(where: { $0.id == loop.id }) { loops[i].active.toggle() }
        await client.setLoopActive(loop, active: !loop.active)
        await loadLoops()
    }

    func createDefaultLoop() async {
        guard connection == .connected else { return }
        let prompt = composerText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let tid = await ensureActiveThread() else { return }
        let useDefaultPrompt = prompt.isEmpty
        do {
            _ = try await client.createSchedule(
                threadId: tid,
                prompt: useDefaultPrompt ? "Default loop prompt" : prompt,
                promptSource: useDefaultPrompt ? "default" : "inline",
                schedule: AppServerClient.dynamicScheduleSpec()
            )
            if !useDefaultPrompt { composerText = "" }
            await loadLoops()
        } catch {
            if prompt.isEmpty {
                composerText = "Loop: "
            }
        }
    }

    func deleteLoop(_ loop: LoopInfo) async {
        guard connection == .connected, !loop.threadId.isEmpty else { return }
        _ = await client.deleteLoop(loop)
        await loadLoops()
    }

    func runLoopNow(_ loop: LoopInfo) async {
        guard connection == .connected, loop.kind == .schedule else { return }
        _ = await client.runLoopNow(loop)
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
            projects = ProjectInfo.derive(from: threads)
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

    // MARK: Search (local, over loaded data)

    private var q: String { searchQuery.trimmingCharacters(in: .whitespaces).lowercased() }
    var searchThreads: [ThreadInfo] {
        if q.isEmpty { return [] }
        return remoteSearchThreads.isEmpty ? threads.filter { $0.name.lowercased().contains(q) } : remoteSearchThreads
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
        loops = (try? await client.listLoops(threadIds: threads.map(\.id))) ?? []
    }

    func loadActiveGoal() async {
        guard connection == .connected, let activeThreadId else {
            activeGoal = nil
            return
        }
        activeGoal = try? await client.getThreadGoal(threadId: activeThreadId)
    }

    func loadAccount() async {
        if let a = try? await client.readAccount() { account = a }
    }

    func loadConfig() async {
        if let cfg = try? await client.readFullConfig() {
            if let configModel = cfg.model { model = configModel }
            if let configProvider = cfg.provider { provider = configProvider }
            if let wireEffort = cfg.effort { effort = Self.displayEffort(wireEffort) }
            configApproval = cfg.approval
            configSandbox = cfg.sandbox
            fullAccess = cfg.sandbox == "danger-full-access"
        }
    }

    func loadProfiles() async {
        if connection == .connected,
           let profiles = try? await client.listAuthProfiles() {
            authProfiles = profiles
            return
        }
        authProfiles = await ProfileRunner.loadProfiles()
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
            if let urlStr = r["authUrl"]?.string, let url = URL(string: urlStr) {
                #if canImport(AppKit)
                NSWorkspace.shared.open(url)
                #endif
            } else if let urlStr = r["verificationUrl"]?.string, let url = URL(string: urlStr) {
                #if canImport(AppKit)
                NSWorkspace.shared.open(url)
                #endif
            }
        } catch {
            loginError = error.localizedDescription
            loginInProgress = false
        }
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
            await client.writeConfig(keyPath: "model_provider", value: .string(Self.providerID(for: providerName)))
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
        await client.writeConfig(keyPath: "model_provider", value: .string(Self.providerID(for: providerName)))
        await loadConfig()
        await loadModelCatalog()
        await loadAccount()
        loginInProgress = false
        if !isSignedIn {
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
        var switched = false
        if connection == .connected,
           (try? await client.switchAuthProfile(name)) != nil {
            switched = true
        }
        if !switched {
            await ProfileRunner.switchProfile(name)
        }
        await reconnectAppServer()
        await loadProfiles()
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
        activeTurnId = nil
        activeMessages = []
        pendingServerRequests = []
        currentProjectPath = t.cwd
        route = .chat(t.id); sidebarSelection = t.name; showSettings = false
        // Prefer resume (so future turns can continue the thread); fall back to a
        // plain read if resume isn't available. `await` can't live in a `??`
        // autoclosure, so branch explicitly.
        let messages: [ChatMessage]
        if let resumed = try? await client.resumeThreadMessages(id: t.id) {
            messages = resumed
        } else {
            messages = (try? await client.readThreadMessages(id: t.id)) ?? []
        }
        guard activeThreadId == requestedThreadId else { return }
        activeMessages = messages
        await loadActiveGoal()
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
        composerText = ""; activeThreadId = nil; activeTurnId = nil; activeGoal = nil; activeMessages = []; pendingServerRequests = []
        open(.home, label: (path as NSString).lastPathComponent)
    }

    /// Threads belonging to a project, by repo-identity group key.
    func threads(forProjectKey key: String) -> [ThreadInfo] {
        threads.filter { ($0.projectKey ?? $0.cwd ?? "") == key }
    }
    func project(forKey key: String) -> ProjectInfo? {
        projects.first { $0.groupKey == key }
    }

    /// The label for the header project selector.
    var currentProjectLabel: String {
        guard let path = currentProjectPath else { return "All projects" }
        return projects.first { $0.path == path }?.name ?? (path as NSString).lastPathComponent
    }

    /// Select the project context for new sessions (nil = all projects / machines).
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
        composerText = ""; activeThreadId = nil; activeTurnId = nil; activeGoal = nil; activeMessages = []; pendingServerRequests = []
        currentProjectPath = nil
        open(.home, label: "New chat")
    }

    // MARK: Sending

    func submitComposer() async {
        let text = composerText.trimmingCharacters(in: .whitespacesAndNewlines)
        // Guard against a second submit while a turn is already running (prevents
        // the same prompt being sent twice via Enter + button or rapid repeats).
        guard !text.isEmpty, connection == .connected, !turnInProgress else { return }
        turnInProgress = true   // set immediately to block re-entry across the awaits below
        streamingAssistantIndex = nil
        activeTurnId = nil
        pendingServerRequests = []
        composerText = ""
        // Show the user's message immediately for responsiveness.
        activeMessages.append(ChatMessage(role: .user, text: text))
        guard let tid = await ensureActiveThread() else {
            finishTurn(failureMessage: "Couldn't start a session. Is the app-server connected?")
            return
        }
        if text.lowercased().hasPrefix("goal:") {
            let objective = Self.goalObjective(from: text)
            if !objective.isEmpty {
                activeGoal = try? await client.setThreadGoal(threadId: tid, objective: objective)
            }
        }
        route = .chat(tid)
        do {
            let turnId = try await client.startTurn(threadId: tid, input: text, model: model, provider: provider,
                                                    effort: Self.wireEffort(effort))
            if activeThreadId == tid {
                activeTurnId = turnId
                startTurnWatchdog()
            }
        } catch {
            finishTurn(failureMessage: error.localizedDescription)
        }
    }

    private func ensureActiveThread() async -> String? {
        if let activeThreadId { return activeThreadId }
        activeThreadId = try? await client.startThread(cwd: currentProjectPath ?? NSHomeDirectory())
        if activeThreadId != nil { await loadThreads(reset: true) }
        return activeThreadId
    }

    func interrupt() async {
        guard let tid = activeThreadId, let turnId = activeTurnId else { return }
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
        pendingServerRequests.removeAll()
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
        let threadAtArm = activeThreadId
        turnWatchdog = Task { @MainActor [weak self] in
            while true {
                try? await Task.sleep(nanoseconds: 5 * 1_000_000_000)
                guard let self, !Task.isCancelled else { return }
                guard self.turnInProgress, self.activeThreadId == threadAtArm else { return }
                if Date().timeIntervalSince(self.lastTurnActivity) >= Self.turnSilenceTimeout {
                    self.finishTurn(failureMessage: "The agent didn't respond in time. It may be stuck — try sending again.")
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
        case "turn/started", "thread/turn/started":
            guard notificationBelongsToActiveThread(params) else { return }
            turnInProgress = true
            activeTurnId = params["turn"]?["id"]?.string ?? params["turnId"]?.string ?? activeTurnId
            startTurnWatchdog()
        case "turn/completed", "thread/turn/completed":
            guard notificationBelongsToActiveThread(params) else { return }
            finishTurn(failureMessage: Self.turnFailureMessage(params))
        case "turn/failed", "thread/turn/failed":
            // Not a real wire method per the schema, but handle defensively.
            guard notificationBelongsToActiveThread(params) else { return }
            finishTurn(failureMessage: Self.turnFailureMessage(params) ?? "The turn failed.")
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
            Task { await loadThreads(reset: true) }
        case "thread/name/updated", "thread/status/changed", "thread/metadata/updated":
            Task { await loadThreads(reset: true) }
        case "thread/settings/updated":
            Task { await loadConfig(); await loadModelCatalog() }
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
        case "thread/goal/cleared":
            guard notificationBelongsToActiveThread(params) else { return }
            activeGoal = nil
        case "app/list/updated":
            let updated = AppServerClient.parseApps(params["data"]?.array ?? [])
            if updated.isEmpty { Task { await loadApps() } } else { apps = updated }
        case "serverRequest/resolved":
            guard notificationBelongsToActiveThread(params) else { return }
            let resolvedId = params["requestId"] ?? params["id"]
            if let resolvedId {
                pendingServerRequests.removeAll { $0.requestId == resolvedId }
            } else {
                pendingServerRequests.removeAll()
            }
        default:
            break
        }
    }

    private func notificationBelongsToActiveThread(_ params: JSONValue) -> Bool {
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
        guard notificationBelongsToActiveThread(request.params) else {
            client.respondError(to: request.id, message: "CodeWith.app is not displaying this thread.")
            return
        }

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
                method: request.method,
                kind: .commandApproval,
                title: "Approve command?",
                detail: detail.isEmpty ? command : detail))
        case "item/fileChange/requestApproval":
            let reason = request.params["reason"]?.string
            let root = request.params["grantRoot"]?.string
            let detail = [reason, root].compactMap { value in
                value?.isEmpty == false ? value : nil
            }.joined(separator: "\n")
            pendingServerRequests.append(PendingServerRequest(
                requestId: request.id,
                method: request.method,
                kind: .fileChangeApproval,
                title: "Approve file changes?",
                detail: detail.isEmpty ? "The agent wants to edit files." : detail))
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
                method: request.method,
                kind: .permissionsApproval,
                title: "Approve permissions?",
                detail: detail.isEmpty ? "The agent wants additional permissions." : detail,
                requestedPermissions: permissions))
        default:
            activeMessages.append(ChatMessage(role: .tool,
                text: "Unsupported app-server request: \(request.method)",
                toolIcon: "exclamationmark.triangle"))
            client.respondError(to: request.id, message: "CodeWith.app does not support \(request.method) yet.")
        }
    }

    func respondToServerRequest(_ prompt: PendingServerRequest, approve: Bool) {
        pendingServerRequests.removeAll { $0.id == prompt.id }
        switch prompt.kind {
        case .commandApproval:
            client.respond(to: prompt.requestId, result: .object([
                "decision": .string(approve ? "accept" : "decline"),
            ]))
        case .fileChangeApproval:
            client.respond(to: prompt.requestId, result: .object([
                "decision": .string(approve ? "accept" : "decline"),
            ]))
        case .permissionsApproval:
            client.respond(to: prompt.requestId, result: .object([
                "scope": .string("turn"),
                "permissions": approve ? (prompt.requestedPermissions ?? .object([:])) : .object([:]),
            ]))
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

    // MARK: Config actions

    func setModel(_ m: String) {
        model = m
        Task { await client.writeConfig(keyPath: "model", value: .string(m)); await loadConfig() }
    }
    func setProvider(_ p: String) {
        provider = p
        Task {
            await client.writeConfig(keyPath: "model_provider", value: .string(p))
            await loadConfig()
            await loadModelCatalog()
        }
    }
    func setEffort(_ e: String) {
        effort = e
        Task { await client.writeConfig(keyPath: "model_reasoning_effort", value: .string(Self.wireEffort(e))) }
    }
    func setApproval(_ a: String) {
        configApproval = a
        Task { await client.writeConfig(keyPath: "approval_policy", value: .string(a)); await loadConfig() }
    }
    func setSandbox(_ s: String) {
        configSandbox = s
        Task { await client.writeConfig(keyPath: "sandbox_mode", value: .string(s)); await loadConfig() }
    }
    func setFullAccess(_ on: Bool) {
        fullAccess = on
        setSandbox(on ? "danger-full-access" : "workspace-write")
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

    // MARK: Add menu

    static let agentNames = ["Apollo", "Ares", "Athena", "Atlas", "Aurelius"]
    func toggleAddMenu() { showAddMenu.toggle() }
    func handleAddAction(_ title: String) {
        switch title {
        case "Plan mode": planMode = true; showAddMenu = false
        case "Goal": prefixGoalComposer()
        case "Files and folders": pickFolder()
        default:
            if Self.agentNames.contains(title) { composerText = "@\(title) " + composerText }
            showAddMenu = false
        }
    }

    private func prefixGoalComposer() {
        if !composerText.hasPrefix("Goal: ") { composerText = "Goal: " + composerText }
        showAddMenu = false
    }

    static func goalObjective(from text: String) -> String {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.lowercased().hasPrefix("goal:") {
            return String(trimmed.dropFirst(5)).trimmingCharacters(in: .whitespacesAndNewlines)
        }
        return trimmed
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
