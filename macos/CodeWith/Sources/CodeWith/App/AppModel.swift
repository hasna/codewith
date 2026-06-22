import SwiftUI

// MARK: - Domain

struct ProjectItem: Identifiable, Hashable {
    let id = UUID()
    var name: String
    var tasks: [TaskRef]
}

struct TaskRef: Identifiable, Hashable {
    let id = UUID()
    var title: String
    var age: String? = nil
    var hasDot: Bool = false
}

struct ChatRef: Identifiable, Hashable {
    let id = UUID()
    var title: String
    var age: String
    var messages: [ChatMessage] = []
}

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

// MARK: - Routing

enum Route: Hashable {
    case home, search, apps, automations, machines, mobile, loops, profiles
    case chat(UUID)
    case task
}

// MARK: - App state

@MainActor
@Observable
final class AppModel {
    var route: Route = .home
    var sidebarSelection: String = "New chat"
    var showSettings = false
    var settingsPage = "General"
    var composerText = ""
    var showAddMenu = false
    var planMode = false
    var fullAccess = true

    var projects: [ProjectItem]
    var chats: [ChatRef]
    var profiles: [ProfileRef]
    var currentProfileID: UUID

    init() {
        projects = [
            ProjectItem(name: "scaffold-api", tasks: [
                TaskRef(title: "Add abstract OAuth prepa…", age: "5mo"),
                TaskRef(title: "Add infra folder for EC2 d…", hasDot: true),
                TaskRef(title: "Add docs folder and files", hasDot: true),
                TaskRef(title: "Find and fix bug in codeba…", hasDot: true),
                TaskRef(title: "Write granular tests (e2e, …", hasDot: true),
            ]),
        ]
        chats = [
            ChatRef(title: "Say hi", age: "1m", messages: [
                ChatMessage(role: .user, text: "hi"),
                ChatMessage(role: .assistant, text: "I'll register the session context first because the provided project rules make that mandatory before any real work. After that I'll keep the response lightweight."),
                ChatMessage(role: .tool, text: "Loaded a tool, ran a command", toolIcon: "wrench.and.screwdriver"),
                ChatMessage(role: .assistant, text: "The first skill path was stale in this environment, so I'm using the installed CodeWith skill location from the session skill list and continuing with the required registration flow."),
                ChatMessage(role: .tool, text: "Reading SKILL.md", toolIcon: "doc.text"),
            ]),
            ChatRef(title: "Ads", age: "3w"),
        ]
        let me = ProfileRef(name: "Andrei Hasna", handle: "@andrei.hasna", plan: "Pro", initials: "AH", colorHex: 0x4AB58E)
        profiles = [
            me,
            ProfileRef(name: "Work", handle: "@work", plan: "Team", initials: "W", colorHex: 0x3B82F6),
            ProfileRef(name: "Personal", handle: "@personal", plan: "Free", initials: "P", colorHex: 0xE9A23B),
        ]
        currentProfileID = me.id
    }

    var currentProfile: ProfileRef { profiles.first { $0.id == currentProfileID } ?? profiles[0] }

    func open(_ r: Route, label: String) {
        route = r
        sidebarSelection = label
        showSettings = false
    }

    func openChat(_ chat: ChatRef) {
        route = .chat(chat.id)
        sidebarSelection = chat.title
        showSettings = false
    }

    func openSettings(_ page: String = "General") {
        showSettings = true
        settingsPage = page
    }

    func newChat() {
        composerText = ""
        open(.home, label: "New chat")
    }

    func submitComposer() {
        let text = composerText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        let chat = ChatRef(title: String(text.prefix(28)), age: "now",
                           messages: [ChatMessage(role: .user, text: text)] + simulatedReply(to: text))
        chats.insert(chat, at: 0)
        composerText = ""
        openChat(chat)
    }

    var liveAgentBusy = false

    /// Runs the live `codewith` agent for the most recent chat and appends its
    /// reply. Falls back gracefully when the CLI is missing or not signed in.
    func requestLiveReply(cwd: String = NSHomeDirectory()) async {
        guard case .chat(let id) = route,
              let idx = chats.firstIndex(where: { $0.id == id }),
              let prompt = chats[idx].messages.last(where: { $0.role == .user })?.text else { return }
        guard AgentRunner.isAvailable else { return }
        liveAgentBusy = true
        let outcome = await AgentRunner.run(prompt: prompt, cwd: cwd)
        liveAgentBusy = false
        guard let i = chats.firstIndex(where: { $0.id == id }) else { return }
        switch outcome {
        case .reply(let text):
            chats[i].messages.append(ChatMessage(role: .assistant, text: text))
        case .notAuthenticated:
            chats[i].messages.append(ChatMessage(role: .assistant,
                text: "⚠︎ Live agent is installed but not signed in. Run `codewith login` on this machine to enable real replies."))
        case .unavailable, .failed:
            break // keep the simulated reply already shown
        }
    }

    /// A lightweight canned working-session reply so a new chat reads like a
    /// real session when no live agent backend is available.
    func simulatedReply(to prompt: String) -> [ChatMessage] {
        [
            ChatMessage(role: .assistant, text: "Got it — working on “\(prompt)”. Let me scan the workspace and plan the change."),
            ChatMessage(role: .tool, text: "Searched the codebase", toolIcon: "magnifyingglass"),
            ChatMessage(role: .assistant, text: "I have enough context to proceed. I'll make the edits and run the tests."),
            ChatMessage(role: .tool, text: "Ran 3 commands", toolIcon: "terminal"),
        ]
    }

    func addProject(name: String) {
        let clean = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !clean.isEmpty else { return }
        projects.insert(ProjectItem(name: clean, tasks: []), at: 0)
        open(.home, label: clean)
    }

    func addTask(_ title: String, toProjectNamed name: String) {
        guard let idx = projects.firstIndex(where: { $0.name == name }) else { return }
        projects[idx].tasks.insert(TaskRef(title: title, age: "now", hasDot: true), at: 0)
    }

    func switchProfile(_ id: UUID) { currentProfileID = id }

    static let agentNames = ["Apollo", "Ares", "Athena", "Atlas", "Aurelius"]

    func handleAddAction(_ title: String) {
        switch title {
        case "Goal": setGoalFromComposer()
        case "Plan mode": setPlanMode(true)
        case "Files and folders": pickFolder()
        default:
            if Self.agentNames.contains(title) {
                composerText = "@\(title) " + composerText
            }
            showAddMenu = false
        }
    }

    /// Opens a native folder picker and adds the chosen repo as a project.
    func pickFolder() {
        showAddMenu = false
        #if canImport(AppKit)
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.prompt = "Add Project"
        if panel.runModal() == .OK, let url = panel.url {
            addProject(name: url.lastPathComponent)
        }
        #endif
    }

    func toggleAddMenu() { showAddMenu.toggle() }
    func setPlanMode(_ on: Bool) { planMode = on; showAddMenu = false }
    func setGoalFromComposer() {
        // "Goal" in the Add menu seeds a goal-style prompt prefix.
        if !composerText.hasPrefix("Goal: ") { composerText = "Goal: " + composerText }
        showAddMenu = false
    }
}
