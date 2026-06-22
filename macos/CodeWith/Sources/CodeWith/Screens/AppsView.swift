import SwiftUI

/// The fork's "Apps" gallery (replaces Codex "Plugins"). A searchable grid of
/// app/skill cards drawn from the hasna ecosystem.
struct AppsView: View {
    private struct App: Identifiable {
        let id = UUID()
        let icon: String
        let tint: Color
        let name: String
        let description: String
        let installed: Bool
    }

    private let apps: [App] = [
        App(icon: "envelope.fill", tint: Color(hex: 0x3B82F6), name: "Mail",
            description: "Read and send email from your agent.", installed: true),
        App(icon: "arrow.up.forward.app.fill", tint: Color(hex: 0x34C759), name: "Deploy",
            description: "Ship builds to staging and production.", installed: false),
        App(icon: "photo.fill", tint: Color(hex: 0xAF52DE), name: "Image",
            description: "Generate and edit images on demand.", installed: true),
        App(icon: "magnifyingglass.circle.fill", tint: Color(hex: 0xFF9500), name: "Deep Research",
            description: "Fan-out, fact-checked multi-source reports.", installed: false),
        App(icon: "checkmark.shield.fill", tint: Color(hex: 0xFF3B30), name: "Security Review",
            description: "Audit diffs for vulnerabilities before shipping.", installed: false),
        App(icon: "doc.text.magnifyingglass", tint: Color(hex: 0x5856D6), name: "Search",
            description: "Trigram local search across the workspace.", installed: true),
        App(icon: "brain.head.profile", tint: Color(hex: 0xFF2D55), name: "Memory",
            description: "Persistent cross-session agent memory.", installed: true),
        App(icon: "arrow.triangle.2.circlepath", tint: Color(hex: 0x00C7BE), name: "Loops",
            description: "Recurring tasks and goal automation.", installed: false),
        App(icon: "key.fill", tint: Color(hex: 0xFFCC00), name: "Secrets",
            description: "Fetch credentials from the vault at runtime.", installed: false),
    ]

    private let columns = Array(repeating: GridItem(.flexible(), spacing: 12), count: 3)

    var body: some View {
        VStack(spacing: 0) {
            topBar
            ScrollColumn(spacing: 0) {
                LazyVGrid(columns: columns, spacing: 12) {
                    ForEach(apps) { app in
                        card(app)
                    }
                }
                .padding(.horizontal, 24)
                .padding(.vertical, 20)
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

    private func card(_ app: App) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            RoundedRectangle(cornerRadius: 9, style: .continuous)
                .fill(app.tint.opacity(0.14))
                .frame(width: 36, height: 36)
                .overlay(Image(systemName: app.icon).font(.system(size: 16)).foregroundStyle(app.tint))
            Text(app.name).font(.system(size: 12.5, weight: .semibold)).foregroundStyle(Theme.textPrimary)
            Text(app.description).font(.system(size: 11)).foregroundStyle(Theme.textSecondary)
                .lineLimit(2).fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 6)
            installPill(installed: app.installed)
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
