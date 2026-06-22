import SwiftUI

/// The fork's "Apps" gallery (replaces Codex "Plugins"). A searchable grid of
/// app/skill cards drawn from the hasna ecosystem.
struct AppsView: View {
    var apps: [AppItemInfo] = []

    private let columns = Array(repeating: GridItem(.flexible(), spacing: 12), count: 3)
    private let tints: [UInt32] = [0x3B82F6, 0x34C759, 0xAF52DE, 0xFF9500, 0xFF3B30, 0x5856D6, 0xFF2D55, 0x00C7BE, 0xFFCC00]

    var body: some View {
        VStack(spacing: 0) {
            topBar
            ScrollColumn(spacing: 0) {
                if apps.isEmpty {
                    Text("No apps available.")
                        .font(.system(size: 12)).foregroundStyle(Theme.textTertiary)
                        .padding(.horizontal, 24).padding(.vertical, 20)
                } else {
                    LazyVGrid(columns: columns, spacing: 12) {
                        ForEach(Array(apps.enumerated()), id: \.element.id) { i, app in
                            card(app, tint: Color(hex: tints[i % tints.count]))
                        }
                    }
                    .padding(.horizontal, 24)
                    .padding(.vertical, 20)
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.canvas)
    }

    private var topBar: some View {
        HStack {
            Text("Apps")
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(Theme.textPrimary)
            Spacer()
            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass").font(.system(size: 11)).foregroundStyle(Theme.textTertiary)
                Text("Search apps").font(.system(size: 12)).foregroundStyle(Theme.textTertiary)
            }
            .padding(.horizontal, 10).frame(width: 180, height: 26)
            .background(RoundedRectangle(cornerRadius: 7).fill(Theme.fieldFill)
                .overlay(RoundedRectangle(cornerRadius: 7).strokeBorder(Theme.cardStroke, lineWidth: 1)))
        }
        .frame(height: 38)
        .padding(.horizontal, 16)
        .overlay(alignment: .bottom) { Rectangle().fill(Theme.separator).frame(height: 1) }
    }

    private func card(_ app: AppItemInfo, tint: Color) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            RoundedRectangle(cornerRadius: 9, style: .continuous)
                .fill(tint.opacity(0.14))
                .frame(width: 36, height: 36)
                .overlay(Image(systemName: "square.grid.2x2.fill").font(.system(size: 15)).foregroundStyle(tint))
            Text(app.name).font(.system(size: 12.5, weight: .semibold)).foregroundStyle(Theme.textPrimary).lineLimit(1)
            Text(app.detail).font(.system(size: 11)).foregroundStyle(Theme.textSecondary)
                .lineLimit(2).fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 6)
            installPill(installed: app.enabled)
        }
        .padding(12)
        .frame(height: 138, alignment: .topLeading)
        .frame(maxWidth: .infinity, alignment: .topLeading)
        .background(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .fill(Theme.fieldFill)
                .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Theme.cardStroke, lineWidth: 1))
        )
    }

    private func installPill(installed: Bool) -> some View {
        HStack(spacing: 4) {
            if installed {
                Image(systemName: "checkmark").font(.system(size: 9, weight: .semibold))
            }
            Text(installed ? "Installed" : "Install").font(.system(size: 11, weight: .medium))
        }
        .foregroundStyle(installed ? Theme.textSecondary : .white)
        .padding(.horizontal, 12).frame(height: 24)
        .background(
            Capsule().fill(installed ? Theme.fieldFill : Theme.accent)
                .overlay(installed ? Capsule().strokeBorder(Theme.cardStroke, lineWidth: 1) : nil)
        )
    }
}
