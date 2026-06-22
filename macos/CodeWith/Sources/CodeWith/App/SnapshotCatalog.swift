import SwiftUI

/// The size of the app window's content (matches the reference captures).
enum WindowSize {
    static let app = CGSize(width: 1000, height: 760)
    /// Including the WindowFrame margin/shadow padding (22 per side).
    static let framed = CGSize(width: 1000 + 44, height: 760 + 44)
}

/// Every screen state we render during snapshot mode.
@MainActor
enum SnapshotCatalog {
    static var items: [SnapshotItem] {
        [
            item("01-home") { RootView(selected: "New chat") { HomeView() } },
            item("02-chat") { RootView(selected: "Say hi") { ChatView() } },
            item("03-add-menu") { RootView(selected: "Say hi") { ChatView(showAddMenu: true) } },
            item("04-task-result") { RootView(selected: "Add abstract OAuth prepa…") { TaskResultView() } },
            item("05-settings-general") { SettingsShell(selected: "General") { SettingsGeneral() } },
            item("06-settings-profile") { SettingsShell(selected: "Profile") { SettingsProfile() } },
            item("07-settings-appearance") { SettingsShell(selected: "Appearance") { SettingsAppearance() } },
            item("08-settings-configuration") { SettingsShell(selected: "Configuration") { SettingsConfiguration() } },
            item("09-settings-personalization") { SettingsShell(selected: "Personalization") { SettingsPersonalization() } },
            item("10-task-result-diff") { RootView(selected: "Add abstract OAuth prepa…") { TaskResultView(showDiffPanel: true) } },
            SnapshotItem(name: "11-login", size: WindowSize.framed, view: AnyView(
                WindowFrame(showTrafficLights: false) { LoginView() }
            )),
            // Fork-specific screens
            item("12-machines") { RootView(selected: "Machines") { MachinesView() } },
            item("13-profiles") { RootView(selected: "Profiles") { ProfilesView() } },
            item("14-apps") { RootView(selected: "Apps") { AppsView() } },
            item("15-loops") { RootView(selected: "Automations") { LoopsView() } },
            item("16-automations") { RootView(selected: "Automations") { AutomationsView() } },
            item("17-search") { RootView(selected: "Search") { SearchView() } },
        ]
    }

    private static func item<V: View>(_ name: String, @ViewBuilder _ content: @escaping () -> V) -> SnapshotItem {
        SnapshotItem(name: name, size: WindowSize.framed, view: AnyView(
            WindowFrame { content() }
        ))
    }
}
