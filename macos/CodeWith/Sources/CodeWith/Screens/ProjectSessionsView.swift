import SwiftUI

/// A project's sessions — all chats whose working directory is this project.
struct ProjectSessionsView: View {
    @Bindable var model: AppModel
    var path: String
    var onThread: (ThreadInfo) -> Void = { _ in }

    private var sessions: [ThreadInfo] { model.threads(inProject: path) }
    private var name: String { model.project(forPath: path)?.name ?? (path as NSString).lastPathComponent }

    var body: some View {
        VStack(spacing: 0) {
            // Header
            HStack(spacing: 8) {
                Image(systemName: "folder").font(.system(size: 12)).foregroundStyle(Theme.textSecondary)
                Text(name).font(.system(size: 13, weight: .medium)).foregroundStyle(Theme.textPrimary).lineLimit(1)
                Spacer()
                Button { model.newSessionInProject(path) } label: {
                    HStack(spacing: 5) {
                        Image(systemName: "square.and.pencil").font(.system(size: 10, weight: .semibold))
                        Text("New session").font(.system(size: 11.5, weight: .medium))
                    }
                    .foregroundStyle(.white).padding(.horizontal, 12).frame(height: 26)
                    .background(Capsule().fill(Color(hex: 0x202020))).contentShape(Capsule())
                }
                .buttonStyle(.plain)
            }
            .padding(.horizontal, 16).frame(height: 40)
            Rectangle().fill(Theme.separator).frame(height: 1)

            ScrollColumn(spacing: 0) {
                VStack(alignment: .leading, spacing: 0) {
                    Text(path).font(.system(size: 11)).foregroundStyle(Theme.textTertiary)
                        .padding(.horizontal, 20).padding(.top, 16).padding(.bottom, 4)
                    Text("\(sessions.count) session\(sessions.count == 1 ? "" : "s")")
                        .font(.system(size: 11, weight: .semibold)).tracking(0.4)
                        .foregroundStyle(Theme.textTertiary).padding(.horizontal, 20).padding(.bottom, 8)

                    ForEach(sessions) { t in
                        Button { onThread(t) } label: {
                            HStack(spacing: 10) {
                                Image(systemName: "bubble.left").font(.system(size: 12)).foregroundStyle(Theme.textSecondary).frame(width: 16)
                                Text(t.name).font(.system(size: 13)).foregroundStyle(Theme.textPrimary).lineLimit(1)
                                Spacer()
                                Text(t.ageLabel).font(.system(size: 11)).foregroundStyle(Theme.textTertiary)
                            }
                            .padding(.horizontal, 12).padding(.vertical, 9).contentShape(Rectangle())
                            .background(RoundedRectangle(cornerRadius: 7).fill(Theme.rowHover))
                        }
                        .buttonStyle(.plain)
                        .padding(.horizontal, 16).padding(.bottom, 4)
                    }
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.canvas)
    }
}
