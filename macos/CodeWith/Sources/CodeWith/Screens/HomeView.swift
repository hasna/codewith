import SwiftUI

/// The "What should we work on?" empty state. No connect cards, no rate banner.
struct HomeView: View {
    var composerText: Binding<String>? = nil
    var onSubmit: (() -> Void)? = nil
    var onPlus: (() -> Void)? = nil
    var onToggleConfig: (() -> Void)? = nil

    var body: some View {
        VStack(spacing: 0) {
            // Detail top bar — the single right-sidebar (config) opener.
            HStack {
                Spacer()
                if let onToggleConfig {
                    Button(action: onToggleConfig) {
                        Image(systemName: "sidebar.right").font(.system(size: 13)).foregroundStyle(Theme.textTertiary)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                }
            }
            .frame(height: 40)
            .padding(.horizontal, 16)

            Spacer()

            Text("What should we work on?")
                .font(.system(size: 23, weight: .regular))
                .foregroundStyle(Theme.textPrimary)
                .padding(.bottom, 24)

            Composer(text: composerText, onSubmit: onSubmit, onPlus: onPlus)
                .frame(width: 392)

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

            Spacer()
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.canvas)
    }
}
