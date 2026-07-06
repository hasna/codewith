import SwiftUI

/// The in-session configuration right sidebar: switch model / gateway / effort
/// for the current session without leaving it. Opened by the top-right button.
struct ConfigPanel: View {
    @Bindable var model: AppModel
    @State private var pendingEscalation: PermissionEscalation?

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text("Session").font(.system(size: 12, weight: .medium)).foregroundStyle(Theme.textPrimary)
                Spacer()
                Button { model.showConfigPanel = false } label: {
                    Image(systemName: "xmark").font(.system(size: 11)).foregroundStyle(Theme.textTertiary)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
            .padding(.horizontal, 14).frame(height: 40)
            Rectangle().fill(Theme.separator).frame(height: 1)

            ScrollColumn(alignment: .leading, spacing: 14) {
                pickerRow("Model", value: model.model ?? "default", options: model.availableModels, display: AppModel.displayModel) { model.setModel($0) }
                pickerRow("Gateway", value: model.provider ?? "openai", options: model.availableProviders, display: AppModel.displayProvider) { model.setProvider($0) }
                pickerRow("Effort", value: model.effort, options: model.availableEfforts) { model.setEffort($0) }
                pickerRow("Permissions", value: model.permissionProfileId, options: model.availablePermissionProfiles, display: AppModel.displayPermissionProfile) {
                    if Self.isPermissionEscalation($0) {
                        pendingEscalation = PermissionEscalation(permissionProfileId: $0)
                    } else {
                        model.setPermissionProfile($0)
                    }
                }
                if !model.authProfiles.isEmpty {
                    pickerRow("Auth profile", value: activeAuthProfileName, options: model.authProfiles.map(\.name)) {
                        model.setSessionAuthProfile($0)
                    }
                }

                if let configError = model.configError {
                    warningRow(configError)
                }

                Rectangle().fill(Theme.separator).frame(height: 1).padding(.vertical, 2)

                HStack(spacing: 4) {
                    Image(systemName: "exclamationmark.triangle.fill").font(.system(size: 10)).foregroundStyle(Theme.warning)
                    Text("Full access").font(.system(size: 12, weight: .medium)).foregroundStyle(Theme.warning)
                    Spacer()
                    GlassToggle(on: model.fullAccess) {
                        if model.fullAccess {
                            model.setFullAccess(false)
                        } else {
                            pendingEscalation = PermissionEscalation(permissionProfileId: ":danger-full-access")
                        }
                    }
                    .disabled(!model.canUseFullAccess && !model.fullAccess)
                    .opacity(!model.canUseFullAccess && !model.fullAccess ? 0.45 : 1)
                    .help(model.canUseFullAccess ? "" : "Full access is blocked by managed requirements.")
                }
            }
            .padding(14)

            Spacer()

            // Account footer: tap to manage profiles.
            Button {
                model.showConfigPanel = false
                model.open(.profiles, label: "Profiles")
                Task { await model.loadProfiles() }
            } label: {
                HStack(spacing: 8) {
                    Circle().fill(Theme.accent).frame(width: 22, height: 22)
                        .overlay(Text(profileInitials).font(.system(size: 9, weight: .semibold)).foregroundStyle(Theme.accentForeground))
                    VStack(alignment: .leading, spacing: 1) {
                        Text(model.account.name).font(.system(size: 11.5, weight: .medium)).foregroundStyle(Theme.textPrimary).lineLimit(1)
                        if let profile = model.currentAuthProfile {
                            Text(profile.name).font(.system(size: 10)).foregroundStyle(Theme.textTertiary)
                        } else if !model.account.plan.isEmpty {
                            Text(model.account.plan).font(.system(size: 10)).foregroundStyle(Theme.textTertiary)
                        }
                    }
                    Spacer()
                    Image(systemName: "chevron.right").font(.system(size: 10)).foregroundStyle(Theme.textTertiary)
                }
                .padding(.horizontal, 14).padding(.vertical, 12).contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .overlay(alignment: .top) { Rectangle().fill(Theme.separator).frame(height: 1) }
        }
        .frame(width: 232)
        .background(Theme.canvas)
        .confirmationDialog(
            "Allow full access?",
            isPresented: Binding(
                get: { pendingEscalation != nil },
                set: { if !$0 { pendingEscalation = nil } }
            ),
            titleVisibility: .visible
        ) {
            Button("Allow full access", role: .destructive) {
                if let pendingEscalation {
                    model.setPermissionProfile(pendingEscalation.permissionProfileId)
                }
                pendingEscalation = nil
            }
            Button("Cancel", role: .cancel) { pendingEscalation = nil }
        } message: {
            Text("This lets CodeWith edit any file and use network without approval for this session.")
        }
    }

    private func warningRow(_ text: String) -> some View {
        HStack(alignment: .top, spacing: 6) {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.system(size: 10))
                .foregroundStyle(Theme.warning)
                .padding(.top, 2)
            Text(text)
                .font(.system(size: 11))
                .foregroundStyle(Theme.textSecondary)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 8)
        .background(RoundedRectangle(cornerRadius: 7).fill(Theme.warning.opacity(0.08)))
    }

    private var activeAuthProfileName: String {
        model.sessionAuthProfileName
            ?? model.authProfiles.first(where: { $0.active })?.name
            ?? model.authProfiles.first?.name
            ?? "Profile"
    }

    private var profileInitials: String {
        if let profile = model.currentAuthProfile {
            return String(profile.name.prefix(2)).uppercased()
        }
        return model.account.initials
    }

    private static func isPermissionEscalation(_ profileId: String) -> Bool {
        profileId == ":danger-full-access"
    }

    private func pickerRow(
        _ label: String,
        value: String,
        options: [String],
        display: @escaping (String) -> String = { $0 },
        onSelect: @escaping (String) -> Void
    ) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(label).font(.system(size: 11)).foregroundStyle(Theme.textTertiary)
            Menu {
                ForEach(options, id: \.self) { opt in
                    Button {
                        onSelect(opt)
                    } label: {
                        if opt == value {
                            Label(display(opt), systemImage: "checkmark")
                        } else {
                            Text(display(opt))
                        }
                    }
                }
            } label: {
                HStack(spacing: 6) {
                    Text(display(value)).font(.system(size: 12)).foregroundStyle(Theme.textPrimary).lineLimit(1)
                    Spacer()
                    Image(systemName: "chevron.up.chevron.down").font(.system(size: 8)).foregroundStyle(Theme.textTertiary)
                }
                .padding(.horizontal, 10).frame(height: 30)
                .background(RoundedRectangle(cornerRadius: 7).fill(Theme.fieldFill)
                    .overlay(RoundedRectangle(cornerRadius: 7).strokeBorder(Theme.cardStroke, lineWidth: 1)))
            }
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            .fixedSize(horizontal: false, vertical: true)
        }
    }
}

private struct PermissionEscalation: Identifiable {
    var permissionProfileId: String
    var id: String { permissionProfileId }
}
