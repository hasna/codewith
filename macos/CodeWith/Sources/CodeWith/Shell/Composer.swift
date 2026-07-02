import SwiftUI

/// The prompt composer. The config pills are real controls; the trailing filled
/// action swaps from microphone at rest to send/stop while active.
struct Composer: View {
    var placeholder: String = "Do anything"
    var showSend: Bool = true
    var stopMode: Bool = false
    var text: Binding<String>? = nil
    var model: AppModel? = nil
    var onSubmit: (() -> Void)? = nil
    var onStop: (() -> Void)? = nil
    var onPlus: (() -> Void)? = nil
    var onConfigTap: (() -> Void)? = nil
    var modelLabel: String = "gpt-5.5"
    var effortLabel: String = "Low"
    @Environment(\.snapshotMode) private var snapshot
    @State private var pendingFullAccessProfile: String?

    /// Short model label to match the reference pill (e.g. "gpt-5.5-codex" → "5.5-codex").
    private var shortModel: String {
        modelLabel.hasPrefix("gpt-") ? String(modelLabel.dropFirst(4)) : modelLabel
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(alignment: .top) {
                if let text, !snapshot {
                    TextField(placeholder, text: text, axis: .vertical)
                        .textFieldStyle(.plain)
                        .font(.system(size: 13))
                        .foregroundStyle(Theme.textPrimary)
                        .lineLimit(2...6)
                        .onSubmit { onSubmit?() }
                } else {
                    // Static placeholder (also used in snapshot mode — ImageRenderer
                    // cannot render an NSTextField-backed TextField).
                    Text(text?.wrappedValue.isEmpty == false ? text!.wrappedValue : placeholder)
                        .font(.system(size: 13))
                        .foregroundStyle(text?.wrappedValue.isEmpty == false ? Theme.textPrimary : Theme.textTertiary)
                        .frame(maxWidth: .infinity, minHeight: 36, alignment: .topLeading)
                }
                Spacer()
            }
            .frame(minHeight: 38, alignment: .topLeading)
            .padding(.horizontal, 14).padding(.top, 13).padding(.bottom, 13)

            HStack(spacing: 10) {
                Button { onPlus?() } label: {
                    Image(systemName: "plus")
                        .font(.system(size: 13, weight: .regular))
                        .foregroundStyle(Theme.textTertiary)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .disabled(onPlus == nil)
                .accessibilityLabel("Add")

                if let model {
                    permissionMenu(model)
                }
                Spacer()

                if let model {
                    ViewThatFits(in: .horizontal) {
                        HStack(spacing: 10) {
                            ProjectMenu(model: model, compact: true)
                            modelMenu(model)
                            providerMenu(model)
                            effortMenu(model)
                            if !model.authProfiles.isEmpty {
                                authProfileMenu(model)
                            }
                        }
                        compactSessionButton(model)
                    }
                } else {
                    Button { onConfigTap?() } label: {
                        HStack(spacing: 3) {
                            Text(shortModel).font(.system(size: 11.5)).foregroundStyle(Theme.textSecondary).lineLimit(1)
                            Text(effortLabel).font(.system(size: 11.5)).foregroundStyle(Theme.textTertiary)
                            Image(systemName: "chevron.down").font(.system(size: 8)).foregroundStyle(Theme.textTertiary)
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain).disabled(onConfigTap == nil)
                    .accessibilityLabel("Model and effort")
                }

                if showSend {
                    let hasText = !(text?.wrappedValue.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ?? true)
                    let icon = stopMode ? "stop.fill" : (hasText ? "arrow.up" : "mic.fill")
                    Button {
                        if stopMode {
                            onStop?()
                        } else if hasText {
                            onSubmit?()
                        }
                    } label: {
                        Circle()
                            .fill(Theme.accent)
                            .frame(width: 28, height: 28)
                            .overlay(Image(systemName: icon)
                                .font(.system(size: 10, weight: .bold)).foregroundStyle(Theme.accentForeground))
                            .contentShape(Circle())
                    }
                    .buttonStyle(.plain)
                    .disabled(!stopMode && !hasText)
                    .accessibilityLabel(stopMode ? "Stop" : (hasText ? "Send" : "Send unavailable"))
                }
            }
            .padding(.horizontal, 12).padding(.bottom, 12)
        }
        .background(
            RoundedRectangle(cornerRadius: Theme.cardRadius, style: .continuous)
                .fill(Theme.fieldFill)
                .overlay(RoundedRectangle(cornerRadius: Theme.cardRadius, style: .continuous).strokeBorder(Theme.cardStroke, lineWidth: 1))
        )
        .shadow(color: Theme.accent.opacity(0.05), radius: 18, x: 0, y: 10)
        .confirmationDialog(
            "Allow full access?",
            isPresented: Binding(
                get: { pendingFullAccessProfile != nil },
                set: { if !$0 { pendingFullAccessProfile = nil } }
            ),
            titleVisibility: .visible
        ) {
            Button("Allow full access", role: .destructive) {
                if let profile = pendingFullAccessProfile {
                    model?.setPermissionProfile(profile)
                }
                pendingFullAccessProfile = nil
            }
            Button("Cancel", role: .cancel) { pendingFullAccessProfile = nil }
        } message: {
            Text("This lets CodeWith edit any file and use network without approval for this session.")
        }
    }

    @ViewBuilder
    private func permissionMenu(_ model: AppModel) -> some View {
        let label = pill(AppModel.displayPermissionProfile(model.permissionProfileId), icon: "lock.shield")
        if snapshot {
            label
        } else {
            Menu {
                ForEach(model.availablePermissionProfiles, id: \.self) { profile in
                    Button(AppModel.displayPermissionProfile(profile)) {
                        if profile == ":danger-full-access" && model.permissionProfileId != ":danger-full-access" {
                            pendingFullAccessProfile = profile
                        } else {
                            model.setPermissionProfile(profile)
                        }
                    }
                }
            } label: {
                label
            }
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            .fixedSize()
            .accessibilityLabel("Permissions")
        }
    }

    @ViewBuilder
    private func modelMenu(_ model: AppModel) -> some View {
        let label = pill(AppModel.displayModel(model.model ?? modelLabel))
        if snapshot {
            label
        } else {
            Menu {
                ForEach(model.availableModels, id: \.self) { option in
                    Button(AppModel.displayModel(option)) { model.setModel(option) }
                }
            } label: {
                label
            }
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            .fixedSize()
        }
    }

    @ViewBuilder
    private func providerMenu(_ model: AppModel) -> some View {
        let label = pill(AppModel.displayProvider(model.provider ?? "openai"))
        if snapshot {
            label
        } else {
            Menu {
                ForEach(model.availableProviders, id: \.self) { option in
                    Button(AppModel.displayProvider(option)) { model.setProvider(option) }
                }
            } label: {
                label
            }
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            .fixedSize()
        }
    }

    @ViewBuilder
    private func effortMenu(_ model: AppModel) -> some View {
        let label = pill(model.effort)
        if snapshot {
            label
        } else {
            Menu {
                ForEach(model.availableEfforts, id: \.self) { option in
                    Button(option) { model.setEffort(option) }
                }
            } label: {
                label
            }
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            .fixedSize()
        }
    }

    @ViewBuilder
    private func authProfileMenu(_ model: AppModel) -> some View {
        let label = pill(activeProfileLabel(model), icon: "person.crop.circle")
        if snapshot {
            label
        } else {
            Menu {
                ForEach(model.authProfiles) { profile in
                    Button(profile.name) { model.setSessionAuthProfile(profile.name) }
                }
            } label: {
                label
            }
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            .fixedSize()
        }
    }

    private func compactSessionButton(_ model: AppModel) -> some View {
        Button { onConfigTap?() } label: {
            pill("\(AppModel.displayModel(model.model ?? modelLabel)) · \(model.effort)", icon: "slider.horizontal.3")
        }
        .buttonStyle(.plain)
        .disabled(onConfigTap == nil)
    }

    private func activeProfileLabel(_ model: AppModel) -> String {
        model.sessionAuthProfileName
            ?? model.authProfiles.first(where: { $0.active })?.name
            ?? (model.account.name == "Signed out" ? "Profile" : model.account.name)
    }

    private func pill(_ text: String, icon: String? = nil) -> some View {
        HStack(spacing: 4) {
            if let icon {
                Image(systemName: icon).font(.system(size: 10))
            }
            Text(text)
                .font(.system(size: 11.5, weight: .regular))
                .foregroundStyle(Theme.textSecondary)
                .lineLimit(1)
            Image(systemName: "chevron.down")
                .font(.system(size: 8))
                .foregroundStyle(Theme.textTertiary)
        }
        .contentShape(Rectangle())
        .frame(maxWidth: 120)
    }
}
