import SwiftUI

struct SettingsConfiguration: View {
    var version: String? = nil
    var approval: String? = nil
    var sandbox: String? = nil
    var error: String? = nil
    var approvalOptions: [String] = ["untrusted", "on-failure", "on-request", "never"]
    var sandboxOptions: [String] = ["read-only", "workspace-write", "danger-full-access"]
    var onSetApproval: (String) -> Void = { _ in }
    var onSetSandbox: (String) -> Void = { _ in }
    var onOpenConfig: () -> Void = {}
    var onDiagnose: () -> Void = {}
    @Environment(\.snapshotMode) private var snapshot
    @State private var pendingDangerousApproval: String?
    @State private var pendingDangerousSandbox: String?

    private var approvalLabel: String {
        switch approval {
        case "never": return "Never"
        case "on-failure": return "On failure"
        case "untrusted": return "Untrusted"
        case "granular": return "Custom"
        default: return "On request"
        }
    }
    private var sandboxLabel: String {
        switch sandbox {
        case "workspace-write": return "Workspace write"
        case "danger-full-access": return "Full access"
        default: return "Read only"
        }
    }

    var body: some View {
        SettingsPage(title: "Configuration") {
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 4) {
                    Text("Configure approval policy and sandbox settings").font(.system(size: 11.5)).foregroundStyle(Theme.textSecondary)
                    Text("Learn more").font(.system(size: 11.5)).foregroundStyle(Theme.accent)
                }
                .padding(.top, -10).padding(.bottom, 22)

                SettingsGroupLabel(text: "Custom config.toml settings")
                if let error {
                    warningRow(error)
                        .padding(.bottom, 8)
                }
                HStack {
                    // Borderless dropdown — the reference renders this selector with
                    // no card outline (text + chevron only).
                    Text("User config").font(.system(size: 12)).foregroundStyle(Theme.textPrimary)
                    Spacer()
                    Button(action: onOpenConfig) {
                        HStack(spacing: 4) {
                            Text("Open config.toml").font(.system(size: 12)).foregroundStyle(Theme.textSecondary)
                            Image(systemName: "arrow.up.right").font(.system(size: 9)).foregroundStyle(Theme.textTertiary)
                        }
                    }
                    .buttonStyle(.plain)
                }
                .padding(.bottom, 8)
                card {
                    SettingsRow(title: "Approval policy", subtitle: "Choose when CodeWith asks for approval") {
                        if snapshot { DropdownPill(text: approvalLabel, minWidth: 150) }
                        else {
                            Menu {
                                ForEach(approvalOptions, id: \.self) { v in
                                    Button(v) { selectApproval(v) }
                                }
                            } label: { DropdownPill(text: approvalLabel, minWidth: 150) }
                            .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize()
                        }
                    }
                    SettingsRow(title: "Sandbox settings", subtitle: "Choose how much CodeWith can do when running commands", showDivider: false) {
                        if snapshot { DropdownPill(text: sandboxLabel, minWidth: 150) }
                        else {
                            Menu {
                                ForEach(sandboxOptions, id: \.self) { v in
                                    Button(v) { selectSandbox(v) }
                                }
                            } label: { DropdownPill(text: sandboxLabel, minWidth: 150) }
                            .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize()
                        }
                    }
                }
                .padding(.bottom, 22)

                SettingsGroupLabel(text: "Workspace Dependencies")
                card {
                    SettingsRow(title: "Current version", subtitle: nil) {
                        Text(version ?? "—").font(.system(size: 12)).foregroundStyle(Theme.textSecondary)
                    }
                    SettingsRow(title: "CodeWith dependencies", subtitle: "Allow CodeWith to install and expose bundled Node.js and Python tools") {
                        GlassToggle(on: true).opacity(0.45)
                            .help("Dependency management is not available in CodeWith.app yet.")
                    }
                    SettingsRow(title: "Diagnostics log", subtitle: "Open the local app-server diagnostics log") {
                        pillButton("Open log", icon: "doc.text.magnifyingglass", color: Theme.textPrimary, action: onDiagnose)
                    }
                    SettingsRow(title: "Reset and install Workspace", subtitle: "Deletes the local bundle, downloads it again, and reloads tools", showDivider: false) {
                        pill("Reinstall", icon: "arrow.down.circle", color: Theme.danger).opacity(0.45)
                            .help("Workspace reinstall is not available in CodeWith.app yet.")
                    }
                }
            }
        }
        .confirmationDialog("Allow high-risk setting?", isPresented: dangerousConfirmationBinding) {
            Button("Apply", role: .destructive) {
                if let approval = pendingDangerousApproval {
                    onSetApproval(approval)
                }
                if let sandbox = pendingDangerousSandbox {
                    onSetSandbox(sandbox)
                }
                clearPendingDangerousSetting()
            }
            Button("Cancel", role: .cancel) { clearPendingDangerousSetting() }
        } message: {
            Text("This can let CodeWith run with fewer approval prompts or broader filesystem and network access.")
        }
    }
    /// Groups rows in a subtle bordered card (reference Configuration layout).
    private func card<C: View>(@ViewBuilder _ content: () -> C) -> some View {
        VStack(spacing: 0) { content() }
            .padding(.horizontal, 14).padding(.vertical, 4)
            .background(
                RoundedRectangle(cornerRadius: 10, style: .continuous)
                    .fill(Color.white)
                    .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(Theme.cardStroke, lineWidth: 1))
            )
    }
    private func pill(_ t: String, icon: String, color: Color) -> some View {
        HStack(spacing: 5) {
            Image(systemName: icon).font(.system(size: 10))
            Text(t).font(.system(size: 12, weight: .medium))
        }
            .foregroundStyle(color).padding(.horizontal, 12).frame(height: 28)
            .background(RoundedRectangle(cornerRadius: 7).fill(color.opacity(0.08))
                .overlay(RoundedRectangle(cornerRadius: 7).strokeBorder(color.opacity(0.35), lineWidth: 1)))
    }
    private func pillButton(_ t: String, icon: String, color: Color, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            pill(t, icon: icon, color: color)
        }
        .buttonStyle(.plain)
    }

    private var dangerousConfirmationBinding: Binding<Bool> {
        Binding(
            get: { pendingDangerousApproval != nil || pendingDangerousSandbox != nil },
            set: { isPresented in
                if !isPresented { clearPendingDangerousSetting() }
            }
        )
    }

    private func selectApproval(_ value: String) {
        if Self.requiresConfirmation(approval: value) {
            pendingDangerousApproval = value
            pendingDangerousSandbox = nil
        } else {
            onSetApproval(value)
        }
    }

    private func selectSandbox(_ value: String) {
        if Self.requiresConfirmation(sandbox: value) {
            pendingDangerousSandbox = value
            pendingDangerousApproval = nil
        } else {
            onSetSandbox(value)
        }
    }

    private func clearPendingDangerousSetting() {
        pendingDangerousApproval = nil
        pendingDangerousSandbox = nil
    }

    static func requiresConfirmation(approval: String) -> Bool {
        approval == "never"
    }

    static func requiresConfirmation(sandbox: String) -> Bool {
        sandbox == "danger-full-access"
    }

    private func warningRow(_ text: String) -> some View {
        HStack(alignment: .top, spacing: 6) {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.system(size: 10))
                .foregroundStyle(Theme.warning)
                .padding(.top, 2)
            Text(text)
                .font(.system(size: 11.5))
                .foregroundStyle(Theme.textSecondary)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 8)
        .background(RoundedRectangle(cornerRadius: 7).fill(Theme.warning.opacity(0.08)))
    }
}
