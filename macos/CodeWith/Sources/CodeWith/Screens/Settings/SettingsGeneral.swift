import SwiftUI

struct SettingsGeneral: View {
    var fullAccess: Bool = true
    var sandbox: String? = nil
    var onToggleFullAccess: () -> Void = {}
    @State private var confirmingFullAccess = false

    var body: some View {
        SettingsPage(title: "General") {
            VStack(alignment: .leading, spacing: 0) {
                SettingsGroupLabel(text: "Work mode")
                Text("Choose how much technical detail CodeWith shows")
                    .font(.system(size: 11.5)).foregroundStyle(Theme.textSecondary).padding(.top, -6).padding(.bottom, 12)
                HStack(spacing: 10) {
                    ChoiceCard(icon: "chevron.left.forwardslash.chevron.right", title: "For coding",
                               subtitle: "More technical responses and control", selected: true)
                    ChoiceCard(icon: "sun.max", title: "For everyday work",
                               subtitle: "Same power, less technical detail", selected: false)
                }
                .padding(.bottom, 14)

                SettingsGroupLabel(text: "Permissions")
                SettingsRow(title: "Default permissions",
                            subtitle: "By default, CodeWith can read and edit files in its workspace. It can ask for additional access when needed.") {
                    GlassToggle(on: true)
                }
                SettingsRow(title: "Auto-review",
                            subtitle: "CodeWith can read and edit files in its workspace. Automatically reviews requests for additional access. Auto-review can make mistakes. Learn more about elevated risks.") {
                    GlassToggle(on: true)
                }
                SettingsRow(title: "Full access",
                            subtitle: "When CodeWith runs with full access, it can edit any file on your computer and run commands with network, without your approval. This significantly increases the risk of data loss, leaks, or unexpected behavior. Learn more about elevated risks.",
                            showDivider: false) {
                    GlassToggle(on: fullAccess || sandbox == "danger-full-access") {
                        if fullAccess || sandbox == "danger-full-access" {
                            onToggleFullAccess()
                        } else {
                            confirmingFullAccess = true
                        }
                    }
                }
                .padding(.bottom, 14)

                SettingsGroupLabel(text: "General")
                SettingsRow(title: "Default file open destination", subtitle: "Where files and folders open by default") {
                    DropdownPill(text: "Cursor", icon: "cursorarrow", minWidth: 140)
                }
                SettingsRow(title: "Language", subtitle: "Language for the app UI") {
                    DropdownPill(text: "Auto detect", minWidth: 140)
                }
                SettingsRow(title: "Show in menu bar", subtitle: "Keep CodeWith in the macOS menu bar when the main window is closed") {
                    GlassToggle(on: true)
                }
                SettingsRow(title: "Bottom panel", subtitle: "Show the bottom panel control in the app header", showDivider: false) {
                    GlassToggle(on: false)
                }
            }
        }
        .confirmationDialog("Allow full access?", isPresented: $confirmingFullAccess, titleVisibility: .visible) {
            Button("Allow full access", role: .destructive, action: onToggleFullAccess)
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("This lets CodeWith edit any file and use network without approval.")
        }
    }
}
