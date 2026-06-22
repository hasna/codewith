import SwiftUI

/// A live session view, driven by `model.activeMessages`.
struct ChatView: View {
    @Bindable var model: AppModel
    var threadId: String
    var onSubmit: () -> Void = {}
    var onPlus: () -> Void = {}
    var onAddAction: (String) -> Void = { _ in }
    var onToggleConfig: () -> Void = {}

    private var title: String {
        model.threads.first { $0.id == threadId }?.name ?? "Chat"
    }

    var body: some View {
        VStack(spacing: 0) {
            // Top bar — title + project selector, and the right-sidebar (config) opener.
            HStack(spacing: 8) {
                Text(title).font(.system(size: 13, weight: .medium)).foregroundStyle(Theme.textPrimary).lineLimit(1)
                Image(systemName: "ellipsis").font(.system(size: 12)).foregroundStyle(Theme.textTertiary)
                ProjectMenu(model: model, compact: true)
                Spacer()
                Button(action: onToggleConfig) {
                    Image(systemName: "sidebar.right").font(.system(size: 13)).foregroundStyle(Theme.textTertiary)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
            .padding(.horizontal, 16).frame(height: 40)
            Rectangle().fill(Theme.separator).frame(height: 1)

            // Conversation
            ScrollColumn(alignment: .leading, spacing: 0) {
                if model.activeMessages.isEmpty {
                    Text(model.turnInProgress ? "Working…" : "")
                        .font(.system(size: 12)).foregroundStyle(Theme.textTertiary)
                        .padding(.top, 8)
                } else {
                    ForEach(model.activeMessages) { messageView($0) }
                    if model.turnInProgress {
                        Text("Working…").font(.system(size: 12)).foregroundStyle(Theme.textTertiary).padding(.top, 4)
                    }
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 24).padding(.top, 18)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)

            // Composer
            Composer(placeholder: "Ask for follow-up changes",
                     stopMode: model.turnInProgress,
                     text: $model.composerText, onSubmit: onSubmit,
                     onStop: { Task { await model.interrupt() } },
                     onPlus: onPlus, onConfigTap: onToggleConfig,
                     modelLabel: model.model ?? "gpt-5.5", effortLabel: model.effort)
                .padding(.horizontal, 24).padding(.vertical, 14)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.canvas)
        .overlay(alignment: .bottomLeading) {
            if model.showAddMenu {
                AddMenu(onAction: onAddAction).padding(.leading, 24).padding(.bottom, 68)
            }
        }
    }

    @ViewBuilder
    private func messageView(_ m: ChatMessage) -> some View {
        switch m.role {
        case .user:
            HStack { Spacer()
                Text(m.text).font(.system(size: 13)).foregroundStyle(Theme.textPrimary)
                    .padding(.horizontal, 12).padding(.vertical, 7)
                    .background(RoundedRectangle(cornerRadius: 16).fill(Color(hex: 0xEDEDEF)))
            }
            .padding(.bottom, 16)
        case .assistant:
            Text(m.text).font(.system(size: 13)).foregroundStyle(Theme.textPrimary)
                .fixedSize(horizontal: false, vertical: true).lineSpacing(3)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.bottom, 12)
        case .tool:
            ToolRow(icon: m.toolIcon ?? "wrench.and.screwdriver", text: m.text)
        }
    }
}

struct ToolRow: View {
    var icon: String
    var text: String
    var body: some View {
        // Compact inline chip (hugs content), left-aligned — matches the reference.
        HStack(spacing: 6) {
            Image(systemName: icon).font(.system(size: 10.5)).foregroundStyle(Theme.textTertiary)
            Text(text).font(.system(size: 12)).foregroundStyle(Theme.textSecondary).lineLimit(1)
        }
        .padding(.horizontal, 8).padding(.vertical, 4)
        .background(RoundedRectangle(cornerRadius: 7).fill(Color.black.opacity(0.035)))
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.bottom, 11)
    }
}
