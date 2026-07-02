import SwiftUI

struct SettingsProfile: View {
    var account: AccountInfo? = nil
    var activeProfile: AuthProfileInfo? = nil
    var profiles: [AuthProfileInfo] = []
    var profileError: String? = nil
    var onManageProfiles: () -> Void = {}

    var body: some View {
        SettingsPage(title: "Profile", subtitle: "Manage the signed-in account and auth profile CodeWith uses.") {
            VStack(alignment: .leading, spacing: 18) {
                accountHeader

                VStack(spacing: 0) {
                    SettingsRow(title: "Active profile", subtitle: activeProfileSubtitle) {
                        Button("Manage") { onManageProfiles() }
                            .buttonStyle(.borderless)
                    }
                    SettingsRow(title: "Saved profiles", subtitle: savedProfilesSubtitle, showDivider: profileError != nil) {
                        Text("\(profiles.count)")
                            .font(.system(size: 12, weight: .medium))
                            .foregroundStyle(Theme.textPrimary)
                    }
                    if let profileError {
                        warningRow(profileError)
                    }
                }
                .padding(.horizontal, 14)
                .padding(.vertical, 4)
                .background(
                    RoundedRectangle(cornerRadius: 10, style: .continuous)
                        .fill(Color.white)
                        .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(Theme.cardStroke, lineWidth: 1))
                )
            }
        }
    }

    private var accountHeader: some View {
        HStack(alignment: .center, spacing: 14) {
            Circle()
                .fill(Color(hex: 0x4AB58E))
                .frame(width: 56, height: 56)
                .overlay(Text(account?.initials ?? "?").font(.system(size: 18, weight: .semibold)).foregroundStyle(.white))

            VStack(alignment: .leading, spacing: 3) {
                Text(account?.name ?? "Signed out")
                    .font(.system(size: 18, weight: .semibold))
                    .foregroundStyle(Theme.textPrimary)
                Text(accountSubtitle)
                    .font(.system(size: 12))
                    .foregroundStyle(Theme.textSecondary)
                    .lineLimit(1)
            }
            Spacer()
        }
        .padding(16)
        .background(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .fill(Theme.fieldFill)
                .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(Theme.cardStroke, lineWidth: 1))
        )
    }

    private var accountSubtitle: String {
        guard let account else { return "No account loaded" }
        let parts = [account.email, account.plan]
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
        return parts.isEmpty ? "No OpenAI account is active" : parts.joined(separator: " · ")
    }

    private var activeProfileSubtitle: String {
        guard let activeProfile else {
            return "No saved profile is marked active. Open Profiles to switch or create one."
        }
        let parts = [activeProfile.name, activeProfile.email, activeProfile.plan, activeProfile.provider]
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
        return parts.joined(separator: " · ")
    }

    private var savedProfilesSubtitle: String {
        if profiles.isEmpty { return "No saved auth profiles found." }
        return profiles.map(\.name).joined(separator: ", ")
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
            Spacer(minLength: 0)
        }
        .padding(.vertical, 8)
    }
}
