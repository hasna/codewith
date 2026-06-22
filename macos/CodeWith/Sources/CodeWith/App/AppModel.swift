import SwiftUI

// MARK: - Routing

enum Route: Hashable {
    case home, search, apps, loops, machines, profiles
    case chat(String)   // thread id
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

    // Backend data
    var threads: [ThreadInfo] = []
    var nextCursor: String? = nil
    var loadingThreads = false
    var projects: [ProjectInfo] = []
    var loops: [LoopInfo] = []
    var account = AccountInfo.signedOut

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
            _ = try await client.initialize()
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
        await loadThreads(reset: true)
        await loadLoops()
        _ = await (acct, cfg)
    }

    // MARK: Threads / projects

    func loadThreads(reset: Bool) async {
        guard connection == .connected, !loadingThreads else { return }
        loadingThreads = true
        defer { loadingThreads = false }
        do {
            let cursor = reset ? nil : nextCursor
            let (newThreads, next) = try await client.listThreads(cursor: cursor, limit: 30)
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

    var hasMoreThreads: Bool { nextCursor != nil }

    func loadLoops() async {
        guard connection == .connected else { return }
        // Schedules/monitors are per-thread; aggregate across loaded threads.
        loops = (try? await client.listLoops(threadIds: threads.map(\.id))) ?? []
    }

    func loadAccount() async {
        if let a = try? await client.readAccount() { account = a }
    }

    func loadConfig() async {
        if let (m, p) = try? await client.readModelConfig() {
            if model == nil { model = m }
            if provider == nil { provider = p }
        }
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

    func newChat() {
        composerText = ""; activeThreadId = nil; activeMessages = []
        open(.home, label: "New chat")
    }

    // MARK: Sending

    func submitComposer() async {
        let text = composerText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, connection == .connected else { return }
        composerText = ""
        // Ensure we have a thread.
        if activeThreadId == nil {
            let cwd = FileManager.default.currentDirectoryPath
            activeThreadId = try? await client.startThread(cwd: cwd)
            await loadThreads(reset: true)
        }
        guard let tid = activeThreadId else { return }
        activeMessages.append(ChatMessage(role: .user, text: text))
        route = .chat(tid)
        turnInProgress = true
        streamingAssistantIndex = nil
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
        case "turn/completed", "thread/turn/completed":
            turnInProgress = false
            streamingAssistantIndex = nil
        default:
            break
        }
    }

    private func handleCompletedItem(_ item: JSONValue) {
        guard !item.isNull else { return }
        if item["type"]?.string == "agentMessage" {
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

    func setModel(_ m: String) { model = m }
    func setProvider(_ p: String) { provider = p }
    func setEffort(_ e: String) { effort = e }

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
            projects.insert(ProjectInfo(name: url.lastPathComponent, path: url.path, threadCount: 0), at: 0)
            open(.home, label: url.lastPathComponent)
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
        return m
    }
}
