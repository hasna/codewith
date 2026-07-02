import SwiftUI

/// Live search across loaded sessions, projects, and apps.
struct SearchView: View {
    @Bindable var model: AppModel
    var onThread: (ThreadInfo) -> Void = { _ in }
    var onProject: (ProjectInfo) -> Void = { _ in }
    @Environment(\.snapshotMode) private var snapshot

    var body: some View {
        VStack(spacing: 0) {
            // Search field
            HStack(spacing: 8) {
                Image(systemName: "magnifyingglass").font(.system(size: 13)).foregroundStyle(Theme.textTertiary)
                if snapshot {
                    Text(model.searchQuery.isEmpty ? "Search chats, projects, and apps…" : model.searchQuery)
                        .font(.system(size: 14)).foregroundStyle(model.searchQuery.isEmpty ? Theme.textTertiary : Theme.textPrimary)
                } else {
                    TextField("Search chats, projects, and apps…", text: $model.searchQuery)
                        .textFieldStyle(.plain).font(.system(size: 14)).foregroundStyle(Theme.textPrimary)
                }
                Spacer()
            }
            .padding(.horizontal, 14).frame(height: 44)
            .background(RoundedRectangle(cornerRadius: 9).fill(Theme.fieldFill)
                .overlay(RoundedRectangle(cornerRadius: 9).strokeBorder(Theme.cardStroke, lineWidth: 1)))
            .padding(16)

            ScrollColumn(spacing: 0) {
                VStack(alignment: .leading, spacing: 8) {
                    if model.searchQuery.trimmingCharacters(in: .whitespaces).isEmpty {
                        empty("Search everything", "Find chats, projects, and apps")
                    } else if !model.hasSearchResults {
                        empty("No results", "Nothing matches “\(model.searchQuery)”")
                    } else {
                        if !model.searchProjects.isEmpty {
                            section("Projects")
                            ForEach(model.searchProjects) { p in
                                resultRow(icon: "folder", title: p.name, subtitle: p.path) { onProject(p) }
                            }
                        }
                        if !model.searchThreads.isEmpty {
                            section("Chats")
                            ForEach(model.searchThreads) { t in
                                resultRow(icon: "bubble.left", title: t.name, subtitle: t.cwd ?? "") { onThread(t) }
                            }
                        }
                        if !model.searchApps.isEmpty {
                            section("Apps")
                            ForEach(model.searchApps) { a in
                                resultLabel(icon: "square.grid.2x2", title: a.name, subtitle: a.detail)
                            }
                        }
                    }
                }
                .padding(.horizontal, 16)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.canvas)
        .task(id: model.searchQuery) {
            await model.runSearch()
        }
    }

    private func section(_ t: String) -> some View {
        Text(t.uppercased()).font(.system(size: 11, weight: .semibold)).tracking(0.4)
            .foregroundStyle(Theme.textTertiary).padding(.top, 10).padding(.bottom, 2)
    }
    private func resultRow(icon: String, title: String, subtitle: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            resultContent(icon: icon, title: title, subtitle: subtitle)
        }
        .buttonStyle(.plain)
    }
    private func resultLabel(icon: String, title: String, subtitle: String) -> some View {
        resultContent(icon: icon, title: title, subtitle: subtitle)
    }
    private func resultContent(icon: String, title: String, subtitle: String) -> some View {
        HStack(spacing: 10) {
            Image(systemName: icon).font(.system(size: 12)).foregroundStyle(Theme.textSecondary).frame(width: 16)
            VStack(alignment: .leading, spacing: 1) {
                Text(title).font(.system(size: 13)).foregroundStyle(Theme.textPrimary).lineLimit(1)
                if !subtitle.isEmpty {
                    Text(subtitle).font(.system(size: 11)).foregroundStyle(Theme.textTertiary).lineLimit(1)
                }
            }
            Spacer()
        }
        .padding(.horizontal, 8).padding(.vertical, 7)
        .background(RoundedRectangle(cornerRadius: 7).fill(Theme.rowHover))
    }
    private func empty(_ title: String, _ sub: String) -> some View {
        VStack(spacing: 6) {
            Image(systemName: "magnifyingglass").font(.system(size: 24)).foregroundStyle(Theme.textTertiary)
            Text(title).font(.system(size: 14, weight: .medium)).foregroundStyle(Theme.textSecondary)
            Text(sub).font(.system(size: 12)).foregroundStyle(Theme.textTertiary)
        }
        .frame(maxWidth: .infinity).padding(.top, 60)
    }
}
