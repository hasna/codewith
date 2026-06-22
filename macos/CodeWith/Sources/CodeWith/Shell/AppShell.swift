import SwiftUI

/// The live application root. Owns the `AppModel`, connects to the app-server on
/// appear, renders the sidebar + detail + optional in-session config panel.
struct AppShell: View {
    @State private var model = AppModel()

    var body: some View {
        Group {
            if model.showSettings {
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
        .task { await model.bootstrap() }
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
                case .search:   SearchView()
                case .apps:     AppsView(apps: model.apps)
                case .loops:    LoopsView(loops: model.loops)
                case .machines: MachinesView()
                case .profiles: ProfilesView()
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
        case "New chat": model.newChat()
        case "Search":   model.open(.search, label: title)
        case "Apps":     model.open(.apps, label: title)
        case "Loops":    model.open(.loops, label: title); Task { await model.loadLoops() }
        case "Machines": model.open(.machines, label: title)
        case "Settings": model.openSettings()
        default:         break
        }
    }

    @ViewBuilder private func settingsPage(_ name: String) -> some View {
        switch name {
        case "Profile":         SettingsProfile(account: model.account)
        case "Appearance":      SettingsAppearance()
        case "Configuration":   SettingsConfiguration()
        case "Personalization": SettingsPersonalization()
        default:                SettingsGeneral()
        }
    }
}

// MARK: - Simple detail panes

struct SearchView: View {
    var body: some View {
        VStack(spacing: 0) {
            DetailTopBar(title: "Search")
            Spacer()
            VStack(spacing: 8) {
                Image(systemName: "magnifyingglass").font(.system(size: 26)).foregroundStyle(Theme.textTertiary)
                Text("Search everything").font(.system(size: 14, weight: .medium)).foregroundStyle(Theme.textSecondary)
                Text("Find chats, projects, machines, and apps").font(.system(size: 12)).foregroundStyle(Theme.textTertiary)
            }
            Spacer()
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.canvas)
    }
}

/// Shared detail top bar with optional right-side config toggle.
struct DetailTopBar: View {
    var title: String
    var onToggleConfig: (() -> Void)? = nil
    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Text(title).font(.system(size: 13, weight: .medium)).foregroundStyle(Theme.textPrimary)
                Spacer()
                if let onToggleConfig {
                    Button(action: onToggleConfig) {
                        Image(systemName: "sidebar.right").font(.system(size: 13)).foregroundStyle(Theme.textTertiary)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(.horizontal, 16).frame(height: 40)
            Rectangle().fill(Theme.separator).frame(height: 1)
        }
    }
}
