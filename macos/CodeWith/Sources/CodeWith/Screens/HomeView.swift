import SwiftUI

/// The "What should we work on?" empty state (reference screenshot 01).
struct HomeView: View {
    var composerText: Binding<String>? = nil
    var onSubmit: (() -> Void)? = nil

    var body: some View {
        VStack(spacing: 0) {
            // Detail top bar (panel toggle on the right).
            HStack {
                Spacer()
                Image(systemName: "square.righthalf.filled")
                    .font(.system(size: 13))
                    .foregroundStyle(Theme.textTertiary)
            }
            .frame(height: 38)
            .padding(.horizontal, 16)

            Spacer()

            Text("What should we work on?")
                .font(.system(size: 23, weight: .regular))
                .foregroundStyle(Theme.textPrimary)
                .padding(.bottom, 24)

            Composer(text: composerText, onSubmit: onSubmit)
                .frame(width: 392)

            // "Work in a project" chip
            HStack(spacing: 5) {
                Image(systemName: "folder").font(.system(size: 10))
                Text("Work in a project").font(.system(size: 11))
                Image(systemName: "chevron.down").font(.system(size: 8))
            }
            .foregroundStyle(Theme.textSecondary)
            .padding(.horizontal, 9).padding(.vertical, 5)
            .background(Capsule().fill(Theme.fieldFill).overlay(Capsule().strokeBorder(Theme.cardStroke, lineWidth: 1)))
            .padding(.top, 12)
            .frame(width: 392, alignment: .leading)

            // Connect cards
            HStack(spacing: 12) {
                connectCard(icon: "message.fill", iconColor: Color(hex: 0x34C759),
                            title: "Connect messaging", subtitle: "Catch up on engineering threads", check: false)
                connectCard(icon: "chevron.left.forwardslash.chevron.right", iconColor: Theme.textPrimary,
                            title: "Connect GitHub", subtitle: "Review PRs, code, and checks", check: true)
                connectCard(icon: "circle.grid.2x2.fill", iconColor: Theme.textPrimary,
                            title: "Connect Linear", subtitle: "Track bugs and implementation work", check: false)
            }
            .padding(.top, 26)

            Spacer()

            // Rate-limit banner
            HStack(spacing: 12) {
                Image(systemName: "clock.arrow.circlepath").font(.system(size: 16)).foregroundStyle(Theme.textSecondary)
                VStack(alignment: .leading, spacing: 2) {
                    Text("You have a new rate limit reset available").font(.system(size: 12, weight: .medium)).foregroundStyle(Theme.textPrimary)
                    Text("You were granted a rate limit reset that will expire in 30 days.").font(.system(size: 11.5)).foregroundStyle(Theme.textSecondary)
                }
                Spacer()
                Text("See resets")
                    .font(.system(size: 11.5, weight: .medium)).foregroundStyle(.white)
                    .padding(.horizontal, 12).padding(.vertical, 6)
                    .background(Capsule().fill(Color(hex: 0x202020)))
                Image(systemName: "xmark").font(.system(size: 10)).foregroundStyle(Theme.textTertiary)
            }
            .padding(.horizontal, 16).padding(.vertical, 12)
            .background(
                RoundedRectangle(cornerRadius: 12, style: .continuous)
                    .fill(Color.white)
                    .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Theme.cardStroke, lineWidth: 1))
                    .shadow(color: .black.opacity(0.05), radius: 8, y: 3)
            )
            .frame(width: 580)
            .padding(.bottom, 22)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.canvas)
    }

    private func connectCard(icon: String, iconColor: Color, title: String, subtitle: String, check: Bool) -> some View {
        VStack(alignment: .leading, spacing: 7) {
            HStack {
                Image(systemName: icon).font(.system(size: 12)).foregroundStyle(iconColor)
                Spacer()
                if check {
                    Image(systemName: "checkmark.circle.fill").font(.system(size: 12)).foregroundStyle(Theme.success)
                }
            }
            Spacer(minLength: 10)
            Text(title).font(.system(size: 11.5, weight: .semibold)).foregroundStyle(Theme.textPrimary)
            Text(subtitle).font(.system(size: 10.5)).foregroundStyle(Theme.textSecondary).lineLimit(2).fixedSize(horizontal: false, vertical: true)
        }
        .padding(11)
        .frame(width: 189, height: 80, alignment: .topLeading)
        .background(
            RoundedRectangle(cornerRadius: 11, style: .continuous)
                .fill(Color.white)
                .overlay(RoundedRectangle(cornerRadius: 11, style: .continuous).strokeBorder(Color(hex: 0xE5E5E5), lineWidth: 1))
        )
    }
}
