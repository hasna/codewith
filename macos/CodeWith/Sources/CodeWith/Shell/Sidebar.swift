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

/// Left navigation rail, driven by live app-server data.
struct Sidebar: View {
    var model: AppModel
    var onTap: (String) -> Void = { _ in }
    var onThread: (ThreadInfo) -> Void = { _ in }
    var onProject: (ProjectInfo) -> Void = { _ in }
    var onLoadMore: () -> Void = {}
    @Environment(\.snapshotMode) private var snapshot

    private let topItems: [SidebarItem] = [
        .init(icon: "house", title: "Home"),
        .init(icon: "square.and.pencil", title: "New chat"),
        .init(icon: "magnifyingglass", title: "Search"),
        .init(icon: "square.grid.2x2", title: "Apps"),
        .init(icon: "arrow.trianglehead.2.clockwise.rotate.90", title: "Loops"),
        .init(icon: "target", title: "Goals"),
        .init(icon: "point.3.connected.trianglepath.dotted", title: "Workflows"),
    ]

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            // Minimal top inset to clear the window traffic-light row.
            Color.clear.frame(height: 28)

            HStack(spacing: 9) {
                BrandMark()
                Text("CodeWith")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(Theme.textPrimary)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 16)
            .padding(.top, 2)
            .padding(.bottom, 12)

            ScrollColumn(alignment: .leading, spacing: 1) {
                ForEach(topItems) { row(for: $0) }

                if !model.machines.isEmpty {
                    sectionHeader("Machine")
                    machineSelector()
                    if let warning = model.pendingMachineSwitchWarning {
                        emptyHint(warning)
                    }
                }

                if !model.projects.isEmpty {
                    sectionHeader("Projects")
                    ForEach(model.projects) { project in
                        projectRow(project)
                    }
                }

                sectionHeader("Chats")
                if model.machineScopedThreads.isEmpty {
                    emptyHint(model.connection == .connecting ? "Loading…" : "No sessions yet")
                } else {
                    ForEach(model.machineScopedThreads) { thread in
                        threadRow(thread)
                    }
                    if model.hasMoreThreads {
                        Button(action: onLoadMore) {
                            Text(model.loadingThreads ? "Loading…" : "Show more")
                                .font(Theme.sidebarItem).foregroundStyle(Theme.textTertiary)
                                .padding(.leading, 8).padding(.vertical, 4)
                                .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 8)
            .padding(.top, 2)

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

    private func emptyHint(_ t: String) -> some View {
        Text(t).font(Theme.sidebarItem).foregroundStyle(Theme.textTertiary)
            .padding(.leading, 8).padding(.vertical, 4)
    }

    private func row(for item: SidebarItem) -> some View {
        let isSel = item.selected || item.title == model.sidebarSelection
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
            }
            .padding(.leading, 8).padding(.trailing, 8)
            .frame(height: 26)
            .contentShape(Rectangle())
            .background(RoundedRectangle(cornerRadius: Theme.rowRadius, style: .continuous).fill(isSel ? Theme.rowSelected : .clear))
        }
        .buttonStyle(.plain)
    }

    private func projectRow(_ project: ProjectInfo) -> some View {
        let isSel = project.name == model.sidebarSelection
        return Button { onProject(project) } label: {
            HStack(spacing: 8) {
                Image(systemName: "folder").font(.system(size: 12.5)).foregroundStyle(isSel ? Theme.textPrimary : Theme.textSecondary).frame(width: 16)
                Text(project.name).font(Theme.sidebarItem).foregroundStyle(isSel ? Theme.textPrimary : Theme.textSecondary).lineLimit(1)
                Spacer(minLength: 4)
            }
            .padding(.horizontal, 8).frame(height: 26).contentShape(Rectangle())
            .background(RoundedRectangle(cornerRadius: Theme.rowRadius, style: .continuous).fill(isSel ? Theme.rowSelected : .clear))
        }
        .buttonStyle(.plain)
    }

    @ViewBuilder
    private func machineSelector() -> some View {
        let label = HStack(spacing: 8) {
            Image(systemName: "desktopcomputer")
                .font(.system(size: 12.5))
                .foregroundStyle(Theme.textSecondary)
                .frame(width: 16)
            Text(model.currentMachineLabel)
                .font(Theme.sidebarItem)
                .foregroundStyle(Theme.textSecondary)
                .lineLimit(1)
            Spacer(minLength: 4)
            Image(systemName: "chevron.up.chevron.down")
                .font(.system(size: 8))
                .foregroundStyle(Theme.textTertiary)
        }
        .padding(.horizontal, 8)
        .frame(height: 26)
        .contentShape(Rectangle())

        if snapshot {
            label
        } else {
            Menu {
                ForEach(model.machines) { machine in
                    Button(machine.displayName) { model.selectMachine(machine) }
                }
            } label: {
                label
            }
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func threadRow(_ thread: ThreadInfo) -> some View {
        let isSel = thread.id == model.activeThreadId
        return Button { onThread(thread) } label: {
            HStack(spacing: 6) {
                Text(thread.name).font(Theme.sidebarItem).foregroundStyle(isSel ? Theme.textPrimary : Theme.textSecondary).lineLimit(1)
                Spacer(minLength: 4)
                Text(thread.ageLabel).font(.system(size: 10.5)).foregroundStyle(Theme.textTertiary)
            }
            .padding(.horizontal, 8).frame(height: 26).contentShape(Rectangle())
            .background(RoundedRectangle(cornerRadius: Theme.rowRadius, style: .continuous).fill(isSel ? Theme.rowSelected : .clear))
        }
        .buttonStyle(.plain)
    }
}

struct BrandMark: View {
    var body: some View {
        RoundedRectangle(cornerRadius: 7, style: .continuous)
            .fill(Theme.accent)
            .frame(width: 26, height: 26)
            .overlay(
                Image(systemName: "chevron.left.forwardslash.chevron.right")
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundStyle(Theme.accentForeground)
            )
    }
}
