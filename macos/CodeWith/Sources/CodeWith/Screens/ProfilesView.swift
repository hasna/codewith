import SwiftUI

/// Profile switcher — a fork-specific screen for switching between accounts /
/// profiles. Self-contained sample data, no AppModel dependency.
struct ProfilesView: View {
    var profiles: [AuthProfileInfo] = []
    var activeEmail: String = ""
    var onSwitch: (String) -> Void = { _ in }

    private let avatarColors: [UInt32] = [0x4AB58E, 0x3B82F6, 0xE9943B, 0x6E6BF2, 0xDB5B5B]

    var body: some View {
        VStack(spacing: 0) {
            // Detail top bar.
            HStack {
                Text("Profiles").font(.system(size: 13)).foregroundStyle(Theme.textSecondary)
                Spacer()
                newPill
            }
            .padding(.horizontal, 22).frame(height: 40)
            Rectangle().fill(Theme.separator).frame(height: 1)

            ScrollColumn(spacing: 0) {
                Text("Switch the profile CodeWith uses for new threads.")
                    .font(.system(size: 12)).foregroundStyle(Theme.textSecondary)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 28).padding(.top, 20).padding(.bottom, 14)

                if profiles.isEmpty {
                    Text("No profiles found.")
                        .font(.system(size: 12)).foregroundStyle(Theme.textTertiary)
                        .padding(.horizontal, 28).padding(.bottom, 8)
                }
                // Profile list (grouped card).
                VStack(spacing: 0) {
                    ForEach(Array(profiles.enumerated()), id: \.element.id) { i, p in
                        profileRow(p, color: Color(hex: avatarColors[i % avatarColors.count]))
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
                .padding(.horizontal, 28)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.canvas)
    }

    private var newPill: some View {
        HStack(spacing: 5) {
            Image(systemName: "plus").font(.system(size: 10, weight: .semibold))
            Text("New profile").font(.system(size: 11.5, weight: .medium))
        }
        .foregroundStyle(.white)
        .padding(.horizontal, 12).frame(height: 26)
        .background(Capsule().fill(Color(hex: 0x202020)))
    }

    private func profileRow(_ p: AuthProfileInfo, color: Color) -> some View {
        let active = p.active || (!activeEmail.isEmpty && p.email == activeEmail)
        let initials = String(p.name.prefix(2)).uppercased()
        return Button { if !active { onSwitch(p.name) } } label: {
            HStack(spacing: 12) {
                Circle().fill(color)
                    .frame(width: 36, height: 36)
                    .overlay(Text(initials).font(.system(size: 14, weight: .semibold)).foregroundStyle(.white))

                VStack(alignment: .leading, spacing: 2) {
                    Text(p.name).font(.system(size: 13, weight: .medium)).foregroundStyle(Theme.textPrimary)
                    HStack(spacing: 6) {
                        Text(p.email).font(.system(size: 11.5)).foregroundStyle(Theme.textSecondary).lineLimit(1)
                        if !p.plan.isEmpty {
                            Text("·").font(.system(size: 11.5)).foregroundStyle(Theme.textTertiary)
                            Text(active ? "\(p.plan) · Active" : p.plan)
                                .font(.system(size: 11.5)).foregroundStyle(Theme.textTertiary)
                        }
                    }
                }
                Spacer()
                if active {
                    Image(systemName: "checkmark.circle.fill").font(.system(size: 16)).foregroundStyle(Theme.toggleBlue)
                }
            }
            .padding(.horizontal, 14).padding(.vertical, 11).contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}
