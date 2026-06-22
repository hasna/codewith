import SwiftUI

struct ChatView: View {
    var showAddMenu: Bool = false
    var chat: ChatRef? = nil
    var composerText: Binding<String>? = nil
    var onSubmit: (() -> Void)? = nil
    var onPlus: (() -> Void)? = nil
    var onAddAction: ((String) -> Void)? = nil

    var body: some View {
        VStack(spacing: 0) {
            // Detail top bar
            HStack(spacing: 8) {
                Text(chat?.title ?? "Say hi").font(.system(size: 13, weight: .medium)).foregroundStyle(Theme.textPrimary)
                Image(systemName: "ellipsis").font(.system(size: 12)).foregroundStyle(Theme.textTertiary)
                Spacer()
                Image(systemName: "rectangle.split.3x1").font(.system(size: 12)).foregroundStyle(Theme.textTertiary)
                Image(systemName: "square.righthalf.filled").font(.system(size: 13)).foregroundStyle(Theme.textTertiary)
            }
            .padding(.horizontal, 16).frame(height: 40)
            Rectangle().fill(Theme.separator).frame(height: 1)

            HStack(spacing: 0) {
                // Conversation column
                VStack(alignment: .leading, spacing: 0) {
                    ScrollColumn(alignment: .leading, spacing: 0) {
                        if let chat, !chat.messages.isEmpty {
                            ForEach(chat.messages) { messageView($0) }
                        } else {
                            // user bubble
                            HStack { Spacer()
                                Text("hi").font(.system(size: 13)).foregroundStyle(Theme.textPrimary)
                                    .padding(.horizontal, 12).padding(.vertical, 7)
                                    .background(RoundedRectangle(cornerRadius: 14).fill(Theme.fieldFill))
                            }
                            .padding(.bottom, 4)
                            HStack { Spacer(); Image(systemName: "link").font(.system(size: 10)).foregroundStyle(Theme.textTertiary) }
                                .padding(.bottom, 16)

                            Text("Working for 8s").font(.system(size: 12)).foregroundStyle(Theme.textTertiary).padding(.bottom, 12)
                            para("I'll register the session context first because the provided project rules make that mandatory before any real work. After that I'll keep the response lightweight.")
                            ToolRow(icon: "wrench.and.screwdriver", text: "Loaded a tool, ran a command")
                            para("The first skill path was stale in this environment, so I'm using the installed CodeWith skill location from the session skill list and continuing with the required registration flow.")
                            ToolRow(icon: "doc.text", text: "Reading SKILL.md")
                        }
                        Spacer(minLength: 0)
                    }
                    .padding(.horizontal, 24).padding(.top, 18)

                    // Composer
                    Composer(placeholder: "Ask for follow-up changes", stopMode: composerText == nil,
                             text: composerText, onSubmit: onSubmit, onPlus: onPlus)
                        .padding(.horizontal, 24).padding(.vertical, 14)
                }
                .frame(maxWidth: .infinity)
                .overlay(alignment: .bottomLeading) {
                    if showAddMenu { AddMenu(onAction: onAddAction ?? { _ in }).padding(.leading, 24).padding(.bottom, 68) }
                }

                // Right panel
                Rectangle().fill(Theme.separator).frame(width: 1)
                VStack(alignment: .leading, spacing: 0) {
                    VStack(alignment: .leading, spacing: 0) {
                        panelSection("Outputs", empty: "No artifacts yet")
                        panelSection("Sources", empty: "No sources yet")
                    }
                    .padding(12)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(RoundedRectangle(cornerRadius: 10).fill(Color.white)
                        .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(Theme.cardStroke, lineWidth: 1)))
                    Spacer()
                }
                .frame(width: 168)
                .padding(.horizontal, 12).padding(.top, 14)
                .background(Color(hex: 0xFAFAFA))
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.canvas)
    }

    @ViewBuilder
    private func messageView(_ m: ChatMessage) -> some View {
        switch m.role {
        case .user:
            HStack { Spacer()
                Text(m.text).font(.system(size: 13)).foregroundStyle(Theme.textPrimary)
                    .padding(.horizontal, 12).padding(.vertical, 7)
                    .background(RoundedRectangle(cornerRadius: 14).fill(Theme.fieldFill))
            }
            .padding(.bottom, 16)
        case .assistant:
            para(m.text)
        case .tool:
            ToolRow(icon: m.toolIcon ?? "wrench.and.screwdriver", text: m.text)
        }
    }

    private func para(_ t: String) -> some View {
        Text(t).font(.system(size: 13)).foregroundStyle(Theme.textPrimary)
            .fixedSize(horizontal: false, vertical: true).lineSpacing(3)
            .padding(.bottom, 12)
    }
    private func panelSection(_ title: String, empty: String) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title).font(.system(size: 12, weight: .medium)).foregroundStyle(Theme.textPrimary)
            Text(empty).font(.system(size: 11.5)).foregroundStyle(Theme.textTertiary)
        }
        .padding(.bottom, 18)
    }
}

struct ToolRow: View {
    var icon: String
    var text: String
    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: icon).font(.system(size: 11)).foregroundStyle(Theme.textTertiary)
            Text(text).font(.system(size: 12)).foregroundStyle(Theme.textSecondary)
            Spacer()
        }
        .padding(.horizontal, 10).padding(.vertical, 7)
        .background(RoundedRectangle(cornerRadius: 8).fill(Color.black.opacity(0.02)))
        .padding(.bottom, 12)
    }
}
