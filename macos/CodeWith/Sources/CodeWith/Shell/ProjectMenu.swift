import SwiftUI

/// Header project selector for the currently selected machine.
struct ProjectMenu: View {
    @Bindable var model: AppModel
    var compact: Bool = false
    @Environment(\.snapshotMode) private var snapshot

    private var label: some View {
        HStack(spacing: 5) {
            Image(systemName: "folder").font(.system(size: compact ? 10 : 11)).foregroundStyle(Theme.textSecondary)
            Text(model.currentProjectLabel).font(.system(size: compact ? 11 : 12)).foregroundStyle(Theme.textSecondary).lineLimit(1)
            Image(systemName: "chevron.down").font(.system(size: 8)).foregroundStyle(Theme.textTertiary)
        }
        .padding(.horizontal, 9).padding(.vertical, 5)
        .background(Capsule().fill(Theme.fieldFill).overlay(Capsule().strokeBorder(Theme.cardStroke, lineWidth: 1)))
        .contentShape(Capsule())
    }

    var body: some View {
        if snapshot {
            label   // ImageRenderer can't draw an NSMenu-backed control.
        } else {
            Menu {
                Button("All projects") { model.selectProject(nil) }
                if !model.projects.isEmpty { Divider() }
                ForEach(model.projects) { p in
                    Button("\(p.name)  ·  \(p.threadCount) session\(p.threadCount == 1 ? "" : "s")") {
                        model.selectProject(p)
                    }
                }
            } label: { label }
            .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize()
        }
    }
}
