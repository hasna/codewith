import SwiftUI

/// Profile switcher — a fork-specific screen for switching between accounts /
/// profiles. Self-contained sample data, no AppModel dependency.
struct ProfilesView: View {
    private struct Profile: Identifiable {
        let id = UUID()
        let name: String
        let handle: String
        let plan: String
        let initials: String
        let color: Color
        let active: Bool
    }

    private let profiles: [Profile] = [
        Profile(name: "Andrei Hasna", handle: "@andrei.hasna", plan: "Pro",
                initials: "AH", color: Color(hex: 0x4AB58E), active: true),
        Profile(name: "Work", handle: "@work", plan: "Team",
                initials: "W", color: Color(hex: 0x3B82F6), active: false),
        Profile(name: "Personal", handle: "@personal", plan: "Free",
                initials: "P", color: Color(hex: 0xE9943B), active: false),
    ]

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

                // Profile list (grouped card).
                VStack(spacing: 0) {
                    ForEach(Array(profiles.enumerated()), id: \.element.id) { i, p in
                        profileRow(p)
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

                // Manage hint row.
                HStack(spacing: 8) {
                    Image(systemName: "gearshape").font(.system(size: 11)).foregroundStyle(Theme.textTertiary)
                    Text("Manage profiles")
                        .font(.system(size: 12)).foregroundStyle(Theme.textSecondary)
                    Spacer()
                    Image(systemName: "chevron.right").font(.system(size: 10)).foregroundStyle(Theme.textTertiary)
                }
                .padding(.horizontal, 14).frame(height: 38)
                .background(
                    RoundedRectangle(cornerRadius: Theme.rowRadius, style: .continuous)
                        .fill(Theme.rowHover)
                )
                .padding(.horizontal, 28).padding(.top, 14)
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

    private func profileRow(_ p: Profile) -> some View {
        HStack(spacing: 12) {
            // Avatar.
            Circle().fill(p.color)
                .frame(width: 36, height: 36)
                .overlay(
                    Text(p.initials)
                        .font(.system(size: 14, weight: .semibold)).foregroundStyle(.white)
                )

            VStack(alignment: .leading, spacing: 2) {
                Text(p.name).font(.system(size: 13, weight: .medium)).foregroundStyle(Theme.textPrimary)
                HStack(spacing: 6) {
                    Text(p.handle).font(.system(size: 11.5)).foregroundStyle(Theme.textSecondary)
                    Text("·").font(.system(size: 11.5)).foregroundStyle(Theme.textTertiary)
                    Text(p.active ? "\(p.plan) · Default" : p.plan)
                        .font(.system(size: 11.5)).foregroundStyle(Theme.textTertiary)
                }
            }

            Spacer()

            if p.active {
                Image(systemName: "checkmark.circle.fill")
                    .font(.system(size: 16)).foregroundStyle(Theme.toggleBlue)
            }
        }
        .padding(.horizontal, 14).padding(.vertical, 11)
    }
}
