import SwiftUI

struct SettingsGeneral: View {
    var fullAccess: Bool = true
    var sandbox: String? = nil
    var desktopSettings = DesktopSettingsInfo()
    var allowFullAccess: Bool = true
    var onToggleFullAccess: () -> Void = {}
    var onSetWorkMode: (String) -> Void = { _ in }
    var onSetFileOpenDestination: (String) -> Void = { _ in }
    var onSetLanguage: (String) -> Void = { _ in }
    var onSetShowMenuBar: (Bool) -> Void = { _ in }
    var onSetBottomPanel: (Bool) -> Void = { _ in }
    @State private var confirmingFullAccess = false

    var body: some View {
        SettingsPage(title: "General") {
            VStack(alignment: .leading, spacing: 0) {
                SettingsGroupLabel(text: "Work mode")
                Text("Choose how much technical detail CodeWith shows")
                    .font(.system(size: 11.5)).foregroundStyle(Theme.textSecondary).padding(.top, -6).padding(.bottom, 12)
                HStack(spacing: 10) {
                    Button {
                        onSetWorkMode("coding")
                    } label: {
                        ChoiceCard(icon: "chevron.left.forwardslash.chevron.right", title: "For coding",
                                   subtitle: "More technical responses and control", selected: desktopSettings.workMode != "everyday")
                    }
                    .buttonStyle(.plain)
                    Button {
                        onSetWorkMode("everyday")
                    } label: {
                        ChoiceCard(icon: "sun.max", title: "For everyday work",
                                   subtitle: "Same power, less technical detail", selected: desktopSettings.workMode == "everyday")
                    }
                    .buttonStyle(.plain)
                }
                .padding(.bottom, 14)

                SettingsGroupLabel(text: "Permissions")
                SettingsRow(title: "Default permissions",
                            subtitle: "By default, CodeWith can read and edit files in its workspace. It can ask for additional access when needed.") {
                    GlassToggle(on: true)
                        .opacity(0.45)
                        .help("Use Approval policy and Sandbox settings in Configuration to change permissions.")
                }
                SettingsRow(title: "Auto-review",
                            subtitle: "CodeWith can read and edit files in its workspace. Automatically reviews requests for additional access. Auto-review can make mistakes. Learn more about elevated risks.") {
                    GlassToggle(on: true)
                        .opacity(0.45)
                        .help("Auto-review is controlled by the CodeWith approval workflow.")
                }
                SettingsRow(title: "Full access",
                            subtitle: "When CodeWith runs with full access, it can edit any file on your computer and run commands with network, without your approval. This significantly increases the risk of data loss, leaks, or unexpected behavior. Learn more about elevated risks.",
                            showDivider: false) {
                    let enabled = fullAccess || sandbox == "danger-full-access"
                    GlassToggle(on: enabled) { toggleFullAccess() }
                        .disabled(!allowFullAccess && !enabled)
                        .opacity(!allowFullAccess && !enabled ? 0.45 : 1)
                        .help(allowFullAccess ? "" : "Full access is blocked by managed requirements.")
                }
                .padding(.bottom, 14)

                SettingsGroupLabel(text: "General")
                SettingsRow(title: "Default file open destination", subtitle: "Where files and folders open by default") {
                    Menu {
                        Button("Cursor") { onSetFileOpenDestination("cursor") }
                        Button("Finder") { onSetFileOpenDestination("finder") }
                        Button("System default") { onSetFileOpenDestination("system") }
                    } label: {
                        DropdownPill(text: fileOpenDestinationLabel, icon: "cursorarrow", minWidth: 140)
                    }
                    .menuStyle(.borderlessButton)
                    .menuIndicator(.hidden)
                    .fixedSize()
                }
                SettingsRow(title: "Language", subtitle: "Language for the app UI") {
                    Menu {
                        Button("Auto detect") { onSetLanguage("auto") }
                        Button("English") { onSetLanguage("en") }
                    } label: {
                        DropdownPill(text: languageLabel, minWidth: 140)
                    }
                    .menuStyle(.borderlessButton)
                    .menuIndicator(.hidden)
                    .fixedSize()
                }
                SettingsRow(title: "Show in menu bar", subtitle: "Keep CodeWith in the macOS menu bar when the main window is closed") {
                    GlassToggle(on: desktopSettings.showMenuBar) { onSetShowMenuBar(!desktopSettings.showMenuBar) }
                }
                SettingsRow(title: "Bottom panel", subtitle: "Show the bottom panel control in the app header", showDivider: false) {
                    GlassToggle(on: desktopSettings.bottomPanel) { onSetBottomPanel(!desktopSettings.bottomPanel) }
                }
            }
        }
        .confirmationDialog("Allow high-risk setting?", isPresented: $confirmingFullAccess) {
            Button("Apply", role: .destructive) { onToggleFullAccess() }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("This can let CodeWith run with broader filesystem and network access.")
        }
    }

    private func toggleFullAccess() {
        let enabled = fullAccess || sandbox == "danger-full-access"
        guard enabled || allowFullAccess else { return }
        if !enabled, SettingsConfiguration.requiresConfirmation(sandbox: "danger-full-access") {
            confirmingFullAccess = true
        } else {
            onToggleFullAccess()
        }
    }

    private var fileOpenDestinationLabel: String {
        switch desktopSettings.fileOpenDestination {
        case "finder": return "Finder"
        case "system": return "System default"
        default: return "Cursor"
        }
    }

    private var languageLabel: String {
        switch desktopSettings.language {
        case "en": return "English"
        default: return "Auto detect"
        }
    }
}
