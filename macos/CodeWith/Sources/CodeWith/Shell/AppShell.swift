import SwiftUI

/// The live application root. Owns the `AppModel`, connects to the app-server on
/// appear, renders the sidebar + detail + optional in-session config panel.
struct AppShell: View {
    @State private var model = AppModel()

    var body: some View {
        Group {
            if model.connection == .connected && !model.isSignedIn {
                LoginView(model: model)
            } else if model.showSettings {
                SettingsShell(
                    selected: model.settingsPage,
                    onSelect: { model.settingsPage = $0 },
                    onBack: { model.showSettings = false }
                ) { settingsPage(model.settingsPage) }
            } else {
                HStack(spacing: 0) {
                    Sidebar(model: model,
                            onTap: handleTap,
                            onThread: { t in Task { await model.openThread(t) } },
                            onProject: { p in model.openProject(p) },
                            onLoadMore: { Task { await model.loadMoreThreads() } })
                    Rectangle().fill(Theme.separator).frame(width: 1)
                    detail
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.canvas)
        .ignoresSafeArea(.container, edges: .top)   // header sits flush under the title bar
        .task { await model.bootstrap() }
        .onReceive(NotificationCenter.default.publisher(for: .codeWithOpenURL)) { note in
            guard let url = note.object as? URL else { return }
            model.handleDesktopURL(url)
        }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.willTerminateNotification)) { _ in
            model.shutdown()
        }
    }

    // MARK: Detail

    @ViewBuilder private var detail: some View {
        HStack(spacing: 0) {
            Group {
                switch model.route {
                case .home:
                    HomeView(model: model,
                             onSubmit: { Task { await model.submitComposer() } },
                             onToggleConfig: { model.showConfigPanel.toggle() })
                case .chat(let id):
                    ChatView(model: model, threadId: id,
                             onSubmit: { Task { await model.submitComposer() } },
                             onPlus: { model.toggleAddMenu() },
                             onAddAction: { model.handleAddAction($0) },
                             onToggleConfig: { model.showConfigPanel.toggle() })
                case .search:   SearchView(model: model,
                                           onThread: { t in Task { await model.openThread(t) } },
                                           onProject: { model.openProject($0) })
                case .apps:     AppsView(apps: model.apps)
                case .loops:
                    LoopsView(
                        loops: model.loops,
                        onToggle: { l in Task { await model.toggleLoop(l) } },
                        onCreate: { Task { await model.createDefaultLoop() } })
                case .project(let key):
                    ProjectSessionsView(model: model, projectKey: key,
                                        onThread: { t in Task { await model.openThread(t) } })
                case .machines: MachinesView(machines: model.machines)
                case .profiles: ProfilesView(profiles: model.authProfiles,
                                             activeEmail: model.account.email,
                                             onSwitch: { name in Task { await model.switchAuthProfile(name) } })
                }
            }
            .frame(maxWidth: .infinity)

            if model.showConfigPanel {
                Rectangle().fill(Theme.separator).frame(width: 1)
                ConfigPanel(model: model)
            }
        }
    }

    // MARK: Routing

    private func handleTap(_ title: String) {
        switch title {
        case "Home":     model.open(.home, label: title)
        case "New chat": model.newChat()
        case "Search":   model.open(.search, label: title)
        case "Apps":     model.open(.apps, label: title)
        case "Loops":    model.open(.loops, label: title); Task { await model.loadLoops() }
        case "Machines": model.open(.machines, label: title); Task { await model.loadMachines() }
        case "Settings": model.openSettings()
        default:         break
        }
    }

    @ViewBuilder private func settingsPage(_ name: String) -> some View {
        switch name {
        case "Profile":         SettingsProfile(account: model.account)
        case "Appearance":      SettingsAppearance()
        case "Configuration":   SettingsConfiguration(version: model.serverVersion, approval: model.configApproval, sandbox: model.configSandbox,
                                                       onSetApproval: { model.setApproval($0) }, onSetSandbox: { model.setSandbox($0) })
        case "Personalization": SettingsPersonalization()
        default:                SettingsGeneral(fullAccess: model.fullAccess, sandbox: model.configSandbox,
                                                onToggleFullAccess: { model.setFullAccess(!model.fullAccess) })
        }
    }
}
