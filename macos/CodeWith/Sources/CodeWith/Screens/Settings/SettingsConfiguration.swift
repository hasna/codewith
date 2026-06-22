import SwiftUI

struct SettingsConfiguration: View {
    var body: some View {
        SettingsPage(title: "Configuration") {
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 4) {
                    Text("Configure approval policy and sandbox settings").font(.system(size: 11.5)).foregroundStyle(Theme.textSecondary)
                    Text("Learn more").font(.system(size: 11.5)).foregroundStyle(Theme.accent)
                }
                .padding(.top, -10).padding(.bottom, 22)

                SettingsGroupLabel(text: "Custom config.toml settings")
                HStack {
                    DropdownPill(text: "User config")
                    Spacer()
                    HStack(spacing: 4) {
                        Text("Open config.toml").font(.system(size: 12)).foregroundStyle(Theme.textSecondary)
                        Image(systemName: "arrow.up.right").font(.system(size: 9)).foregroundStyle(Theme.textTertiary)
                    }
                }
                .padding(.bottom, 6)
                SettingsRow(title: "Approval policy", subtitle: "Choose when CodeWith asks for approval") {
                    DropdownPill(text: "On request")
                }
                SettingsRow(title: "Sandbox settings", subtitle: "Choose how much CodeWith can do when running commands", showDivider: false) {
                    DropdownPill(text: "Read only")
                }
                .padding(.bottom, 22)

                SettingsGroupLabel(text: "Workspace Dependencies")
                SettingsRow(title: "Current version", subtitle: nil) {
                    Text("26.619.11828").font(.system(size: 12)).foregroundStyle(Theme.textSecondary)
                }
                SettingsRow(title: "CodeWith dependencies", subtitle: "Allow CodeWith to install and expose bundled Node.js and Python tools") {
                    GlassToggle(on: true)
                }
                SettingsRow(title: "Diagnose issues in CodeWith Workspace", subtitle: "Checks the current bundle and records diagnostic logs") {
                    pill("Diagnose", icon: "magnifyingglass", color: Theme.textPrimary)
                }
                SettingsRow(title: "Reset and install Workspace", subtitle: "Deletes the local bundle, downloads it again, and reloads tools", showDivider: false) {
                    pill("Reinstall", icon: "arrow.down.circle", color: Theme.danger)
                }
            }
        }
    }
    private func pill(_ t: String, icon: String, color: Color) -> some View {
        HStack(spacing: 5) {
            Image(systemName: icon).font(.system(size: 10))
            Text(t).font(.system(size: 12, weight: .medium))
        }
        .foregroundStyle(color).padding(.horizontal, 12).frame(height: 28)
        .background(RoundedRectangle(cornerRadius: 7).strokeBorder(color.opacity(0.4), lineWidth: 1))
    }
}
