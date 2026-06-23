import SwiftUI

/// Profile switcher backed by `authProfile/list`, `authProfile/switch`, and
/// `authProfile/saveCurrent`.
struct ProfilesView: View {
    var profiles: [AuthProfileInfo] = []
    var activeEmail: String = ""
    var profileError: String? = nil
    var loginInProgress: Bool = false
    var loginError: String? = nil
    var onSwitch: (String) -> Void = { _ in }
    var onCreateChatGPT: (String) -> Void = { _ in }
    var onCreateApiKey: (String, String) -> Void = { _, _ in }

    @State private var showingNewProfile = false
    @State private var newProfileName = ""
    @State private var apiKey = ""
    @State private var setupMode = NewProfileSetupMode.chatgpt

    private let avatarColors: [UInt32] = [0x4AB58E, 0x3B82F6, 0xE9943B, 0x6E6BF2, 0xDB5B5B]

    var body: some View {
        VStack(spacing: 0) {
            topBar
            Rectangle().fill(Theme.separator).frame(height: 1)

            ScrollColumn(spacing: 0) {
                VStack(alignment: .leading, spacing: 12) {
                    Text("Switch the auth profile CodeWith uses for new app-server sessions.")
                        .font(.system(size: 12))
                        .foregroundStyle(Theme.textSecondary)

                    if let message = profileError ?? loginError {
                        statusRow(message, icon: "exclamationmark.triangle.fill", color: Theme.warning)
                    }

                    if profiles.isEmpty {
                        statusRow("No saved auth profiles were found.", icon: "person.crop.circle.badge.questionmark", color: Theme.textTertiary)
                    } else {
                        VStack(spacing: 0) {
                            ForEach(Array(profiles.enumerated()), id: \.element.id) { i, profile in
                                profileRow(profile, color: Color(hex: avatarColors[i % avatarColors.count]))
                                if i < profiles.count - 1 {
                                    Rectangle().fill(Theme.separator).frame(height: 1)
                                        .padding(.leading, 60)
                                }
                            }
                        }
                        .background(
                            RoundedRectangle(cornerRadius: Theme.cardRadius, style: .continuous)
                                .fill(Theme.fieldFill)
                                .overlay(
                                    RoundedRectangle(cornerRadius: Theme.cardRadius, style: .continuous)
                                        .strokeBorder(Theme.cardStroke, lineWidth: 1)
                                )
                        )
                    }
                }
                .padding(.horizontal, 28)
                .padding(.vertical, 20)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.canvas)
        .sheet(isPresented: $showingNewProfile) {
            newProfileSheet
                .frame(width: 420)
                .padding(22)
        }
    }

    private var topBar: some View {
        HStack {
            Text("Profiles").font(.system(size: 13)).foregroundStyle(Theme.textSecondary)
            Spacer()
            Button {
                showingNewProfile = true
            } label: {
                HStack(spacing: 5) {
                    Image(systemName: "plus").font(.system(size: 10, weight: .semibold))
                    Text("New profile").font(.system(size: 11.5, weight: .medium))
                }
                .foregroundStyle(.white)
                .padding(.horizontal, 12).frame(height: 26)
                .background(Capsule().fill(Color(hex: 0x202020)))
            }
            .buttonStyle(.plain)
            .disabled(loginInProgress)
        }
        .padding(.horizontal, 22).frame(height: 40)
    }

    private var newProfileSheet: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack {
                Text("New profile")
                    .font(.system(size: 18, weight: .semibold))
                    .foregroundStyle(Theme.textPrimary)
                Spacer()
                Button {
                    showingNewProfile = false
                } label: {
                    Image(systemName: "xmark").font(.system(size: 11, weight: .semibold))
                }
                .buttonStyle(.plain)
            }

            Text("Create a saved auth profile from a ChatGPT login or OpenAI API key.")
                .font(.system(size: 12))
                .foregroundStyle(Theme.textSecondary)

            VStack(alignment: .leading, spacing: 6) {
                Text("Profile name").font(.system(size: 12, weight: .medium)).foregroundStyle(Theme.textPrimary)
                TextField("work", text: $newProfileName)
                    .textFieldStyle(.roundedBorder)
            }

            Picker("", selection: $setupMode) {
                Text("ChatGPT").tag(NewProfileSetupMode.chatgpt)
                Text("API key").tag(NewProfileSetupMode.apiKey)
            }
            .pickerStyle(.segmented)

            if setupMode == .apiKey {
                VStack(alignment: .leading, spacing: 6) {
                    Text("OpenAI API key").font(.system(size: 12, weight: .medium)).foregroundStyle(Theme.textPrimary)
                    SecureField("sk-...", text: $apiKey)
                        .textFieldStyle(.roundedBorder)
                }
            }

            if let message = profileError ?? loginError {
                statusRow(message, icon: "exclamationmark.triangle.fill", color: Theme.warning)
            }

            HStack {
                Spacer()
                Button("Cancel") { showingNewProfile = false }
                Button(primarySetupTitle) {
                    let name = newProfileName.trimmingCharacters(in: .whitespacesAndNewlines)
                    switch setupMode {
                    case .chatgpt:
                        onCreateChatGPT(name)
                    case .apiKey:
                        onCreateApiKey(name, apiKey)
                    }
                    if canSubmitNewProfile {
                        showingNewProfile = false
                    }
                }
                .buttonStyle(.borderedProminent)
                .disabled(!canSubmitNewProfile || loginInProgress)
            }
        }
    }

    private var primarySetupTitle: String {
        if loginInProgress { return "Working..." }
        switch setupMode {
        case .chatgpt: return "Sign in with ChatGPT"
        case .apiKey: return "Save API key profile"
        }
    }

    private var canSubmitNewProfile: Bool {
        let nameReady = !newProfileName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        switch setupMode {
        case .chatgpt:
            return nameReady
        case .apiKey:
            return nameReady && !apiKey.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        }
    }

    private func profileRow(_ profile: AuthProfileInfo, color: Color) -> some View {
        let active = profile.active || (!activeEmail.isEmpty && profile.email == activeEmail)
        let initials = profileInitials(profile)
        return Button { if !active { onSwitch(profile.name) } } label: {
            HStack(spacing: 12) {
                Circle().fill(color)
                    .frame(width: 36, height: 36)
                    .overlay(Text(initials).font(.system(size: 14, weight: .semibold)).foregroundStyle(.white))

                VStack(alignment: .leading, spacing: 2) {
                    Text(profile.name).font(.system(size: 13, weight: .medium)).foregroundStyle(Theme.textPrimary)
                    Text(profileSubtitle(profile, active: active))
                        .font(.system(size: 11.5))
                        .foregroundStyle(Theme.textSecondary)
                        .lineLimit(1)
                }
                Spacer()
                if active {
                    Image(systemName: "checkmark.circle.fill").font(.system(size: 16)).foregroundStyle(Theme.toggleBlue)
                }
            }
            .padding(.horizontal, 14).padding(.vertical, 11).contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(loginInProgress)
    }

    private func profileSubtitle(_ profile: AuthProfileInfo, active: Bool) -> String {
        let parts = [profile.email, profile.plan, profile.provider]
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
        let suffix = active ? ["Active"] : []
        return (parts + suffix).joined(separator: " · ")
    }

    private func profileInitials(_ profile: AuthProfileInfo) -> String {
        let source = profile.name.isEmpty ? profile.email : profile.name
        let parts = source.split { !$0.isLetter && !$0.isNumber }
        let initials = parts.prefix(2).compactMap(\.first).map(String.init).joined().uppercased()
        return initials.isEmpty ? "?" : initials
    }

    private func statusRow(_ text: String, icon: String, color: Color) -> some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: icon).font(.system(size: 12)).foregroundStyle(color).padding(.top, 1)
            Text(text)
                .font(.system(size: 12))
                .foregroundStyle(Theme.textSecondary)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
        }
        .padding(12)
        .background(
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .fill(color.opacity(0.08))
        )
    }
}

private enum NewProfileSetupMode: Hashable {
    case chatgpt
    case apiKey
}
