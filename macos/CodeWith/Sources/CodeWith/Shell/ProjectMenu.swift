import SwiftUI

/// Header project selector — pick the project (repo) to work in for new sessions,
/// across all machines. "All projects" searches across everything.
struct ProjectMenu: View {
    @Bindable var model: AppModel
    var compact: Bool = false

    var body: some View {
        Menu {
            Button("All projects") { model.selectProject(nil) }
            if !model.projects.isEmpty { Divider() }
            ForEach(model.projects) { p in
                Button {
                    model.selectProject(p)
                } label: {
                    Text("\(p.name)  ·  \(p.threadCount) session\(p.threadCount == 1 ? "" : "s")")
                }
            }
        } label: {
            HStack(spacing: 5) {
                Image(systemName: "folder").font(.system(size: compact ? 10 : 11)).foregroundStyle(Theme.textSecondary)
                Text(model.currentProjectLabel).font(.system(size: compact ? 11 : 12)).foregroundStyle(Theme.textSecondary).lineLimit(1)
                Image(systemName: "chevron.down").font(.system(size: 8)).foregroundStyle(Theme.textTertiary)
            }
            .padding(.horizontal, 9).padding(.vertical, 5)
            .background(Capsule().fill(Theme.fieldFill).overlay(Capsule().strokeBorder(Theme.cardStroke, lineWidth: 1)))
            .contentShape(Capsule())
        }
        .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize()
    }
}
