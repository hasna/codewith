import SwiftUI

struct SettingsNavItem: Identifiable {
    let id = UUID()
    var icon: String
    var title: String
    var selected: Bool = false
}

/// Settings window: a dedicated left nav + a scrolling content pane.
struct SettingsShell<Content: View>: View {
    var selected: String
    var onSelect: (String) -> Void = { _ in }
    var onBack: () -> Void = {}
    @ViewBuilder var content: () -> Content

    private var sections: [(String, [SettingsNavItem])] {
        [
            ("Personal", [
                .init(icon: "gearshape", title: "General"),
                .init(icon: "person.crop.circle", title: "Profile"),
                .init(icon: "sun.max", title: "Appearance"),
                .init(icon: "slider.horizontal.3", title: "Configuration"),
                .init(icon: "sparkles", title: "Personalization"),
                .init(icon: "pawprint", title: "Pets"),
                .init(icon: "command", title: "Keyboard shortcuts"),
                .init(icon: "creditcard", title: "Usage & billing"),
            ]),
            ("Integrations", [
                .init(icon: "square.grid.2x2", title: "Appshots"),
                .init(icon: "antenna.radiowaves.left.and.right", title: "MCP servers"),
                .init(icon: "globe", title: "Browser"),
                .init(icon: "cursorarrow.rays", title: "Computer use"),
            ]),
            ("Coding", [
                .init(icon: "link", title: "Hooks"),
                .init(icon: "point.3.connected.trianglepath.dotted", title: "Connections"),
                .init(icon: "arrow.triangle.branch", title: "Git"),
                .init(icon: "cube", title: "Environments"),
                .init(icon: "arrow.triangle.pull", title: "Worktrees"),
            ]),
            ("Archived", [
                .init(icon: "archivebox", title: "Archived chats"),
            ]),
        ]
    }

    var body: some View {
        HStack(spacing: 0) {
            // Settings sidebar
            VStack(alignment: .leading, spacing: 0) {
                Color.clear.frame(height: 36)   // clear traffic lights; brand mark removed

                Button(action: onBack) {
                    HStack(spacing: 6) {
                        Image(systemName: "arrow.left").font(.system(size: 11, weight: .medium))
                        Text("Back to app").font(.system(size: 12))
                        Spacer()
                    }
                    .foregroundStyle(Theme.textSecondary)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .padding(.leading, 14).padding(.bottom, 10)

                // Search field
                HStack(spacing: 6) {
                    Image(systemName: "magnifyingglass").font(.system(size: 11)).foregroundStyle(Theme.textTertiary)
                    Text("Search settings…").font(.system(size: 12)).foregroundStyle(Theme.textTertiary)
                    Spacer()
                }
                .padding(.horizontal, 8).frame(height: 28)
                .background(RoundedRectangle(cornerRadius: 7).fill(Color.black.opacity(0.04)))
                .padding(.horizontal, 10).padding(.bottom, 8)

                ScrollColumn(alignment: .leading, spacing: 1) {
                    ForEach(sections, id: \.0) { section in
                        Text(section.0)
                            .font(Theme.sidebarSection).foregroundStyle(Theme.textTertiary)
                            .padding(.leading, 12).padding(.top, 12).padding(.bottom, 3)
                        ForEach(section.1) { navRow($0) }
                    }
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, 8)
            }
            .frame(width: 215)
            .background(Theme.sidebar)

            Rectangle().fill(Theme.separator).frame(width: 1)

            // Content
            ScrollColumn(alignment: .leading, spacing: 0) {
                content()
            }
            .background(Theme.canvas)
        }
        .background(Theme.canvas)
    }

    private func navRow(_ item: SettingsNavItem) -> some View {
        let isSel = item.title == selected
        return Button { onSelect(item.title) } label: {
            HStack(spacing: 8) {
                Image(systemName: item.icon).font(.system(size: 12)).foregroundStyle(isSel ? Theme.textPrimary : Theme.textSecondary).frame(width: 16)
                Text(item.title).font(.system(size: 12.5)).foregroundStyle(isSel ? Theme.textPrimary : Theme.textSecondary)
                Spacer()
            }
            .padding(.leading, 8).padding(.trailing, 8).frame(height: 26)
            .contentShape(Rectangle())
            .background(RoundedRectangle(cornerRadius: 7).fill(isSel ? Theme.rowSelected : .clear))
        }
        .buttonStyle(.plain)
    }
}
