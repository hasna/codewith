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
        return [
            framed("01-home") { RootView(model: m) { HomeView(model: m) } },
            framed("02-chat") { RootView(model: m) { ChatView(model: m, threadId: "t1") } },
            framed("05-settings-general") { SettingsShell(selected: "General") { SettingsGeneral() } },
            framed("06-settings-profile") { SettingsShell(selected: "Profile") { SettingsProfile() } },
            framed("07-settings-appearance") { SettingsShell(selected: "Appearance") { SettingsAppearance() } },
            framed("08-settings-configuration") { SettingsShell(selected: "Configuration") { SettingsConfiguration() } },
            framed("09-settings-personalization") { SettingsShell(selected: "Personalization") { SettingsPersonalization() } },
            framed("12-machines") { RootView(model: m) { MachinesView() } },
            framed("13-profiles") { RootView(model: m) { ProfilesView() } },
            framed("14-apps") { RootView(model: m) { AppsView() } },
            framed("15-loops") { RootView(model: m) { LoopsView(loops: m.loops) } },
            SnapshotItem(name: "11-login", size: WindowSize.framed, view: AnyView(
                WindowFrame(showTrafficLights: false) { LoginView() }
            )),
        ]
    }

    private static func framed<V: View>(_ name: String, @ViewBuilder _ content: @escaping () -> V) -> SnapshotItem {
        SnapshotItem(name: name, size: WindowSize.framed, view: AnyView(WindowFrame { content() }))
    }
}
