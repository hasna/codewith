import SwiftUI

/// The live, interactive application root. Owns the `AppModel`, renders the
/// sidebar + detail, routes sidebar taps, and presents Settings as a full
/// dedicated view (matching the reference app).
struct AppShell: View {
    @State private var model = AppModel()

    var body: some View {
        Group {
            if model.showSettings {
                SettingsShell(
                    selected: model.settingsPage,
                    onSelect: { model.settingsPage = $0 },
                    onBack: { model.showSettings = false }
                ) {
                    settingsPage(model.settingsPage)
                }
            } else {
                HStack(spacing: 0) {
                    Sidebar(selected: model.sidebarSelection, onTap: handleTap)
                    Rectangle().fill(Theme.separator).frame(width: 1)
                    detail
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.canvas)
    }

    // MARK: Detail routing

    @ViewBuilder private var detail: some View {
        switch model.route {
        case .home:
            HomeView(composerText: $model.composerText, onSubmit: { submit() })
                .onTapGesture { if model.showAddMenu { model.showAddMenu = false } }
        case .search:      SearchView()
        case .apps:        AppsView()
        case .automations: AutomationsView()
        case .machines:    MachinesView()
        case .mobile:      MobileView()
        case .loops:       LoopsView()
        case .profiles:    ProfilesView()
        case .task:        TaskResultView()
        case .chat(let id):
            let chat = model.chats.first { $0.id == id }
            ChatView(showAddMenu: model.showAddMenu, chat: chat,
                     composerText: $model.composerText, onSubmit: { submit() },
                     onPlus: { model.toggleAddMenu() },
                     onAddAction: { model.handleAddAction($0) })
        }
    }

    /// Submit the composer, then ask the live agent for a real reply (no-op if
    /// codewith isn't installed/authenticated — the simulated reply still shows).
    private func submit() {
        model.submitComposer()
        Task { await model.requestLiveReply() }
    }

    // MARK: Sidebar tap routing

    private func handleTap(_ title: String) {
        switch title {
        case "New chat":        model.newChat()
        case "Search":          model.open(.search, label: title)
        case "Apps":            model.open(.apps, label: title)
        case "Automations":     model.open(.automations, label: title)
        case "Machines":        model.open(.machines, label: title)
        case "CodeWith mobile": model.open(.mobile, label: title)
        case "Settings":        model.openSettings()
        case "scaffold-api":    model.open(.home, label: title)
        case "Show more":       break
        default:
            if let chat = model.chats.first(where: { $0.title == title }) {
                model.openChat(chat)
            } else {
                // A project task row → task result.
                model.open(.task, label: title)
            }
        }
    }

    @ViewBuilder private func settingsPage(_ name: String) -> some View {
        switch name {
        case "Profile":         SettingsProfile()
        case "Appearance":      SettingsAppearance()
        case "Configuration":   SettingsConfiguration()
        case "Personalization": SettingsPersonalization()
        default:                SettingsGeneral()
        }
    }
}

/// Simple search detail pane.
struct SearchView: View {
    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "magnifyingglass").font(.system(size: 12)).foregroundStyle(Theme.textTertiary)
                Text("Search chats, projects, and apps…").font(.system(size: 13)).foregroundStyle(Theme.textTertiary)
                Spacer()
            }
            .padding(.horizontal, 14).frame(height: 44)
            .background(RoundedRectangle(cornerRadius: 9).fill(Theme.fieldFill)
                .overlay(RoundedRectangle(cornerRadius: 9).strokeBorder(Theme.cardStroke, lineWidth: 1)))
            .padding(16)
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

struct MobileView: View {
    var body: some View {
        VStack(spacing: 10) {
            Spacer()
            Image(systemName: "iphone").font(.system(size: 34)).foregroundStyle(Theme.accent)
            Text("CodeWith mobile").font(.system(size: 16, weight: .semibold)).foregroundStyle(Theme.textPrimary)
            Text("Scan the QR code in the app to pair your phone\nand keep working on the go.")
                .multilineTextAlignment(.center)
                .font(.system(size: 12)).foregroundStyle(Theme.textSecondary)
            RoundedRectangle(cornerRadius: 12).strokeBorder(Theme.cardStroke, lineWidth: 1)
                .frame(width: 120, height: 120)
                .overlay(Image(systemName: "qrcode").font(.system(size: 64)).foregroundStyle(Theme.textPrimary))
                .padding(.top, 8)
            Spacer()
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.canvas)
    }
}
