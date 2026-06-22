import SwiftUI

struct SidebarItem: Identifiable {
    let id = UUID()
    var icon: String
    var title: String
    var trailing: String? = nil
    var hasCloud: Bool = false
    var hasDot: Bool = false
    var indent: Bool = false
    var selected: Bool = false
}

/// Left navigation rail. Matches the reference Codex sidebar; "Plugins" is
/// renamed to "Apps" and a "Machines" row is added for the fork.
struct Sidebar: View {
    var selected: String = ""
    var onTap: (String) -> Void = { _ in }

    private let topItems: [SidebarItem] = [
        .init(icon: "square.and.pencil", title: "New chat"),
        .init(icon: "magnifyingglass", title: "Search"),
        .init(icon: "square.grid.2x2", title: "Apps"),
        .init(icon: "bolt.horizontal.circle", title: "Automations"),
        .init(icon: "desktopcomputer", title: "Machines"),
        .init(icon: "iphone", title: "CodeWith mobile"),
    ]

    private let projects: [SidebarItem] = [
        .init(icon: "folder", title: "scaffold-api"),
        .init(icon: "doc", title: "Add abstract OAuth prepa…", trailing: "5mo", hasCloud: true, indent: true, selected: true),
        .init(icon: "doc", title: "Add infra folder for EC2 d…", hasCloud: true, hasDot: true, indent: true),
        .init(icon: "doc", title: "Add docs folder and files", hasCloud: true, hasDot: true, indent: true),
        .init(icon: "doc", title: "Find and fix bug in codeba…", hasCloud: true, hasDot: true, indent: true),
        .init(icon: "doc", title: "Write granular tests (e2e, …", hasCloud: true, hasDot: true, indent: true),
    ]

    private let chats: [SidebarItem] = [
        .init(icon: "bubble.left", title: "Say hi", trailing: "1m"),
        .init(icon: "bubble.left", title: "Ads", trailing: "3w"),
    ]

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            // Top chrome row: brand mark + nav chevrons + layout toggle.
            HStack(spacing: 8) {
                Spacer().frame(width: 56) // clear the traffic lights
                BrandMark()
                Image(systemName: "chevron.left").font(.system(size: 11, weight: .medium)).foregroundStyle(Theme.textTertiary)
                Image(systemName: "chevron.right").font(.system(size: 11, weight: .medium)).foregroundStyle(Theme.textTertiary.opacity(0.5))
                Spacer()
            }
            .frame(height: 38)
            .padding(.horizontal, 10)

            ScrollColumn(alignment: .leading, spacing: 1) {
                ForEach(topItems) { row(for: $0) }

                sectionHeader("Projects")
                ForEach(projects) { row(for: $0) }
                HStack(spacing: 4) {
                    Text("Show more")
                        .font(Theme.sidebarItem)
                        .foregroundStyle(Theme.textTertiary)
                }
                .padding(.leading, 30).padding(.vertical, 4)

                sectionHeader("Chats")
                ForEach(chats) { row(for: $0) }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 8)
            .padding(.top, 2)

            Divider().overlay(Theme.separator)
            // Settings pinned to bottom.
            row(for: .init(icon: "gearshape", title: "Settings"))
                .padding(.horizontal, 8).padding(.vertical, 6)
        }
        .frame(width: Theme.sidebarWidth)
        .background(Theme.sidebar)
    }

    private func sectionHeader(_ t: String) -> some View {
        Text(t)
            .font(Theme.sidebarSection)
            .foregroundStyle(Theme.textTertiary)
            .padding(.leading, 8).padding(.top, 14).padding(.bottom, 4)
    }

    private func row(for item: SidebarItem) -> some View {
        let isSel = item.selected || item.title == selected
        return Button { onTap(item.title) } label: {
            HStack(spacing: 8) {
                Image(systemName: item.icon)
                    .font(.system(size: 12.5, weight: .regular))
                    .foregroundStyle(isSel ? Theme.textPrimary : Theme.textSecondary)
                    .frame(width: 16)
                Text(item.title)
                    .font(Theme.sidebarItem)
                    .foregroundStyle(isSel ? Theme.textPrimary : Theme.textSecondary)
                    .lineLimit(1)
                Spacer(minLength: 4)
                if item.hasCloud {
                    Image(systemName: "cloud").font(.system(size: 10)).foregroundStyle(Theme.textTertiary)
                }
                if let t = item.trailing {
                    Text(t).font(.system(size: 10.5)).foregroundStyle(Theme.textTertiary)
                }
                if item.hasDot {
                    Circle().fill(Theme.accent).frame(width: 5, height: 5)
                }
            }
            .padding(.leading, item.indent ? 22 : 8)
            .padding(.trailing, 8)
            .frame(height: 26)
            .contentShape(Rectangle())
            .background(
                RoundedRectangle(cornerRadius: Theme.rowRadius, style: .continuous)
                    .fill(isSel ? Theme.rowSelected : .clear)
            )
        }
        .buttonStyle(.plain)
    }
}

struct BrandMark: View {
    var body: some View {
        RoundedRectangle(cornerRadius: 6, style: .continuous)
            .fill(LinearGradient(colors: [Color(hex: 0x6E6BF2), Color(hex: 0x4B47E0)], startPoint: .top, endPoint: .bottom))
            .frame(width: 30, height: 19)
            .overlay(
                Image(systemName: "chevron.left.forwardslash.chevron.right")
                    .font(.system(size: 9, weight: .bold))
                    .foregroundStyle(.white)
            )
    }
}
