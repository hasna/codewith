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
    var apps: [AppItemInfo] = []
    var machines: [MachineInfo] = []
    var authProfiles: [AuthProfileInfo] = []
    var account = AccountInfo.signedOut
    var serverVersion: String? = nil
    var configApproval: String? = nil
    var configSandbox: String? = nil

    // In-session config
    var model: String? = nil
    var provider: String? = nil
    var effort: String = "Low"
    let availableModels = ["gpt-5.5-codex", "gpt-5.5", "o3", "gpt-4.1"]
    let availableProviders = ["openai", "azure", "openrouter", "ollama"]
    let availableEfforts = ["Low", "Medium", "High", "Extra High"]

    // Active chat
    var activeThreadId: String? = nil
    var activeMessages: [ChatMessage] = []
    var turnInProgress = false
    var currentProjectPath: String? = nil
    private var streamingAssistantIndex: Int? = nil

    // Profiles (local switch; profile picker UI)
    var profiles: [ProfileRef]
    var currentProfileID: UUID

    init() {
        let me = ProfileRef(name: "You", handle: "@me", plan: "", initials: "ME", colorHex: 0x4AB58E)
        profiles = [me]
        currentProfileID = me.id
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
        Task { @MainActor [weak self] in
            guard let stream = self?.client.notifications else { return }
            for await (method, params) in stream {
                self?.handleNotification(method: method, params: params)
            }
        }
    }

    func shutdown() { client.stop() }

    var currentProfile: ProfileRef { profiles.first { $0.id == currentProfileID } ?? profiles[0] }

    // MARK: Bootstrap

    func bootstrap() async {
        guard client.isAvailable else {
            connection = .unavailable("codewith CLI not found"); return
        }
        guard connection == .connecting else { return }   // idempotent
        do {
            try client.start()
            let initResult = try await client.initialize()
            serverVersion = Self.parseVersion(initResult["userAgent"]?.string)
            connection = .connected
        } catch {
            connection = .unavailable(error.localizedDescription); return
        }
        startNotificationConsumer()
        await refreshAll()
    }

    func refreshAll() async {
        async let acct: () = loadAccount()
        async let cfg: () = loadConfig()
        async let apps: () = loadApps()
        await loadThreads(reset: true)          // fast first-page paint
        await loadLoops()
        _ = await (acct, cfg, apps)
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

    func loadMachines() async {
        let (m, _) = await FleetRunner.loadFleet()
        if !m.isEmpty { machines = m }
    }

    func toggleLoop(_ loop: LoopInfo) async {
        guard connection == .connected, !loop.threadId.isEmpty else { return }
        // Optimistic flip, then call backend and reload.
        if let i = loops.firstIndex(where: { $0.id == loop.id }) { loops[i].active.toggle() }
        await client.setLoopActive(loop, active: !loop.active)
        await loadLoops()
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
        q.isEmpty ? [] : threads.filter { $0.name.lowercased().contains(q) }
    }
    var searchProjects: [ProjectInfo] {
        q.isEmpty ? [] : projects.filter { $0.name.lowercased().contains(q) }
    }
    var searchApps: [AppItemInfo] {
        q.isEmpty ? [] : apps.filter { $0.name.lowercased().contains(q) || $0.detail.lowercased().contains(q) }
    }
    var hasSearchResults: Bool { !searchThreads.isEmpty || !searchProjects.isEmpty || !searchApps.isEmpty }

    func loadLoops() async {
        guard connection == .connected else { return }
        // Schedules/monitors are per-thread; aggregate across loaded threads.
        loops = (try? await client.listLoops(threadIds: threads.map(\.id))) ?? []
    }

    func loadAccount() async {
        if let a = try? await client.readAccount() { account = a }
    }

    func loadConfig() async {
        if let cfg = try? await client.readFullConfig() {
            if model == nil { model = cfg.model }
            if provider == nil { provider = cfg.provider }
            configApproval = cfg.approval
            configSandbox = cfg.sandbox
        }
    }

    func loadProfiles() async {
        authProfiles = await ProfileRunner.loadProfiles()
    }

    // MARK: Login / auth

    var loginInProgress = false
    var loginError: String? = nil

    var isSignedIn: Bool {
        let n = account.name
        return n != "Signed out" && !n.isEmpty
    }

    /// Start ChatGPT OAuth: open the returned auth URL in the browser. The
    /// `account/login/completed` notification finalizes it.
    func loginWithChatGPT() async {
        guard connection == .connected else { return }
        loginInProgress = true; loginError = nil
        do {
            let r = try await client.request("account/login/start", .object(["type": .string("chatgpt")]), timeout: 30)
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
    func loginWithApiKey(_ key: String) async {
        let k = key.trimmingCharacters(in: .whitespacesAndNewlines)
        guard connection == .connected, !k.isEmpty else { return }
        loginInProgress = true; loginError = nil
        _ = try? await client.request("account/login/start",
            .object(["type": .string("apiKey"), "apiKey": .string(k)]), timeout: 30)
        await loadAccount()
        loginInProgress = false
        if !isSignedIn { loginError = "That key was not accepted." }
    }

    func cancelLogin() async {
        _ = try? await client.request("account/login/cancel", .object([:]), timeout: 10)
        loginInProgress = false
    }

    func logout() async {
        _ = try? await client.request("account/logout", .object([:]), timeout: 10)
        await loadAccount()
    }

    /// Switch the active CLI profile, then reconnect the session.
    func switchAuthProfile(_ name: String) async {
        await ProfileRunner.switchProfile(name)
        await loadAccount()
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

    // MARK: Navigation

    func open(_ r: Route, label: String) {
        route = r; sidebarSelection = label; showSettings = false; showAddMenu = false
    }

    func openThread(_ t: ThreadInfo) async {
        activeThreadId = t.id
        activeMessages = []
        route = .chat(t.id); sidebarSelection = t.name; showSettings = false
        activeMessages = (try? await client.readThreadMessages(id: t.id)) ?? []
    }

    func openSettings(_ page: String = "General") { showSettings = true; settingsPage = page }

    /// Show a project's sessions (and let the user start a new one there).
    func openProject(_ p: ProjectInfo) {
        currentProjectPath = p.path
        open(.project(p.groupKey), label: p.name)
    }

    /// New session scoped to a project's directory.
    func newSessionInProject(_ path: String) {
        currentProjectPath = path
        composerText = ""; activeThreadId = nil; activeMessages = []
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
        currentProjectPath = p?.path
    }

    func newChat() {
        composerText = ""; activeThreadId = nil; activeMessages = []
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
        composerText = ""
        // Show the user's message immediately for responsiveness.
        activeMessages.append(ChatMessage(role: .user, text: text))
        // Ensure we have a thread (use a real directory, not the app bundle cwd).
        if activeThreadId == nil {
            activeThreadId = try? await client.startThread(cwd: currentProjectPath ?? NSHomeDirectory())
            await loadThreads(reset: true)
        }
        guard let tid = activeThreadId else {
            turnInProgress = false
            activeMessages.append(ChatMessage(role: .assistant, text: "⚠︎ Couldn't start a session. Is the app-server connected?"))
            return
        }
        route = .chat(tid)
        do {
            try await client.startTurn(threadId: tid, input: text, model: model, provider: provider,
                                       effort: effort.lowercased().replacingOccurrences(of: " ", with: ""))
        } catch {
            turnInProgress = false
            activeMessages.append(ChatMessage(role: .assistant,
                text: "⚠︎ \(error.localizedDescription)"))
        }
    }

    func interrupt() async {
        guard let tid = activeThreadId else { return }
        await client.interruptTurn(threadId: tid)
        turnInProgress = false
    }

    // MARK: Live notifications (turn streaming)

    func handleNotification(method: String, params: JSONValue) {
        switch method {
        // Real wire name is "item/agentMessage/delta"; aliases kept defensively.
        case "item/agentMessage/delta", "item/agentMessageDelta", "agentMessageDelta", "thread/agentMessageDelta":
            appendAssistantDelta(params["delta"]?.string ?? params["text"]?.string ?? "")
        case "item/completed", "thread/item/completed", "thread/realtimeItemAdded":
            handleCompletedItem(params["item"] ?? .null)
        case "turn/started", "thread/turn/started":
            turnInProgress = true
        case "turn/completed", "thread/turn/completed", "turn/failed", "thread/turn/failed":
            turnInProgress = false
            streamingAssistantIndex = nil
        case "account/login/completed", "account/updated", "account/login/chatGptComplete":
            loginInProgress = false
            Task { await loadAccount(); await refreshAll() }
        case "thread/started", "thread/closed", "thread/archived", "thread/unarchived":
            // A session appeared/changed — refresh the list so Projects + Chats stay live.
            Task { await loadThreads(reset: true) }
        default:
            break
        }
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
        Task { await client.writeConfig(keyPath: "model_provider", value: .string(p)); await loadConfig() }
    }
    func setEffort(_ e: String) {
        effort = e
        Task { await client.writeConfig(keyPath: "model_reasoning_effort", value: .string(e.lowercased().replacingOccurrences(of: " ", with: ""))) }
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

    // MARK: Add menu

    static let agentNames = ["Apollo", "Ares", "Athena", "Atlas", "Aurelius"]
    func toggleAddMenu() { showAddMenu.toggle() }
    func handleAddAction(_ title: String) {
        switch title {
        case "Plan mode": planMode = true; showAddMenu = false
        case "Goal": if !composerText.hasPrefix("Goal: ") { composerText = "Goal: " + composerText }; showAddMenu = false
        case "Files and folders": pickFolder()
        default:
            if Self.agentNames.contains(title) { composerText = "@\(title) " + composerText }
            showAddMenu = false
        }
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
