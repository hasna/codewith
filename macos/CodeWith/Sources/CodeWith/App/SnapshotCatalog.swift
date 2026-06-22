import SwiftUI

enum WindowSize {
    static let app = CGSize(width: 1000, height: 760)
    static let framed = CGSize(width: 1000 + 44, height: 760 + 44)
}

/// Screens rendered during snapshot mode (parity verification). Uses a
/// disconnected sample model so the real views render representative data.
@MainActor
enum SnapshotCatalog {
    static var items: [SnapshotItem] {
        let m = AppModel.sample()
        let menu = AppModel.sample(); menu.showAddMenu = true   // for the "+" add-menu screen
        let search = AppModel.sample(); search.searchQuery = "auth"; search.sidebarSelection = "Search"
        return [
            framed("01-home") { RootView(model: m) { HomeView(model: m) } },
            framed("02-chat") { RootView(model: m) { ChatView(model: m, threadId: "t1") } },
            framed("03-add-menu") { RootView(model: menu) { HomeView(model: menu) } },
            framed("05-settings-general") { SettingsShell(selected: "General") { SettingsGeneral() } },
            framed("06-settings-profile") { SettingsShell(selected: "Profile") { SettingsProfile(account: m.account) } },
            framed("07-settings-appearance") { SettingsShell(selected: "Appearance") { SettingsAppearance() } },
            framed("08-settings-configuration") { SettingsShell(selected: "Configuration") { SettingsConfiguration(version: m.serverVersion, approval: m.configApproval, sandbox: m.configSandbox) } },
            framed("09-settings-personalization") { SettingsShell(selected: "Personalization") { SettingsPersonalization() } },
            framed("12-machines") { RootView(model: m) { MachinesView(machines: m.machines) } },
            framed("13-profiles") { RootView(model: m) { ProfilesView(profiles: m.authProfiles, activeEmail: m.account.email) } },
            framed("14-apps") { RootView(model: m) { AppsView(apps: m.apps) } },
            framed("15-loops") { RootView(model: m) { LoopsView(loops: m.loops) } },
            framed("17-search") { RootView(model: search) { SearchView(model: search) } },
            SnapshotItem(name: "11-login", size: WindowSize.framed, view: AnyView(
                WindowFrame(showTrafficLights: false) { LoginView(model: m) }
            )),
        ]
    }

    private static func framed<V: View>(_ name: String, @ViewBuilder _ content: @escaping () -> V) -> SnapshotItem {
        SnapshotItem(name: name, size: WindowSize.framed, view: AnyView(WindowFrame { content() }))
    }
}
