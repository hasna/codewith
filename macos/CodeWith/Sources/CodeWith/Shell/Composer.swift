import SwiftUI

/// The prompt composer. Send button, microphone state, and inline config pills
/// are all real, clickable controls.
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

    /// Short model label to match the reference pill (e.g. "gpt-5.5-codex" → "5.5-codex").
    private var shortModel: String {
        modelLabel.hasPrefix("gpt-") ? String(modelLabel.dropFirst(4)) : modelLabel
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack {
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
                }
                Spacer()
            }
            .frame(minHeight: 38, alignment: .topLeading)
            .padding(.horizontal, 14).padding(.top, 10).padding(.bottom, 12)

            HStack(spacing: 10) {
                Button { onPlus?() } label: {
                    Image(systemName: "plus")
                        .font(.system(size: 13, weight: .regular))
                        .foregroundStyle(Theme.textTertiary)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .disabled(onPlus == nil)

                if let model {
                    permissionMenu(model)
                }
                Spacer()

                if let model {
                    ProjectMenu(model: model, compact: true)
                    modelMenu(model)
                    providerMenu(model)
                    effortMenu(model)
                    if !model.authProfiles.isEmpty {
                        authProfileMenu(model)
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
                            .fill(stopMode || hasText ? Color(hex: 0x202020) : Color(hex: 0xBEBEBE))
                            .frame(width: 22, height: 22)
                            .overlay(Image(systemName: icon)
                                .font(.system(size: 10, weight: .bold)).foregroundStyle(.white))
                            .contentShape(Circle())
                    }
                    .buttonStyle(.plain)
                    .disabled(!stopMode && !hasText)
                    .accessibilityLabel(stopMode ? "Stop" : (hasText ? "Send" : "Send unavailable"))
                }
            }
            .padding(.horizontal, 12).padding(.bottom, 10)
        }
        .background(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .fill(Theme.fieldFill)
                .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Theme.cardStroke, lineWidth: 1))
        )
    }

    private func permissionMenu(_ model: AppModel) -> some View {
        Menu {
            ForEach(model.availablePermissionProfiles, id: \.self) { profile in
                Button(AppModel.displayPermissionProfile(profile)) {
                    model.setPermissionProfile(profile)
                }
            }
        } label: {
            pill(AppModel.displayPermissionProfile(model.permissionProfileId), icon: "lock.shield")
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .fixedSize()
    }

    private func modelMenu(_ model: AppModel) -> some View {
        Menu {
            ForEach(model.availableModels, id: \.self) { option in
                Button(AppModel.displayModel(option)) { model.setModel(option) }
            }
        } label: {
            pill(AppModel.displayModel(model.model ?? modelLabel))
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .fixedSize()
    }

    private func providerMenu(_ model: AppModel) -> some View {
        Menu {
            ForEach(model.availableProviders, id: \.self) { option in
                Button(AppModel.displayProvider(option)) { model.setProvider(option) }
            }
        } label: {
            pill(AppModel.displayProvider(model.provider ?? "openai"))
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .fixedSize()
    }

    private func effortMenu(_ model: AppModel) -> some View {
        Menu {
            ForEach(model.availableEfforts, id: \.self) { option in
                Button(option) { model.setEffort(option) }
            }
        } label: {
            pill(model.effort)
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .fixedSize()
    }

    private func authProfileMenu(_ model: AppModel) -> some View {
        Menu {
            ForEach(model.authProfiles) { profile in
                Button(profile.name) { model.setSessionAuthProfile(profile.name) }
            }
        } label: {
            pill(activeProfileLabel(model), icon: "person.crop.circle")
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .fixedSize()
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
