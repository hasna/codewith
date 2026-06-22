import SwiftUI

struct SettingsPersonalization: View {
    private let instructions = """
# Agent Rules for CodeWith

## No Worktrees
Never use git worktrees. Work directly in the repository.

## Never Ask Questions — Just Act
Never stop to ask the user to choose between options, clarify requirements, or
disambiguate. Do not present multiple-choice questions ("which approach?",
"should I do A or B?"). When something is ambiguous, resolve it yourself using
best practices, the codebase, and sensible defaults, state your interpretation in
one line, and continue. Bias entirely toward delivering a result over seeking
confirmation — act and course-correct later.
"""

    var body: some View {
        SettingsPage(title: "Personalization") {
            VStack(alignment: .leading, spacing: 0) {
                SettingsRow(title: "Personality", subtitle: "Choose a default tone for CodeWith responses", showDivider: false) {
                    DropdownPill(text: "Pragmatic")
                }
                .padding(.bottom, 18)

                HStack(spacing: 4) {
                    Text("Custom instructions").font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.textPrimary)
                    Spacer()
                }
                Text("Give CodeWith extra instructions and context for all tasks on this host.")
                    .font(.system(size: 11.5)).foregroundStyle(Theme.textSecondary).padding(.top, 4).padding(.bottom, 8)
                Text(instructions)
                    .font(.system(size: 11, design: .monospaced)).foregroundStyle(Theme.textPrimary)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(12)
                    .frame(height: 120, alignment: .topLeading)
                    .clipped()
                    .background(RoundedRectangle(cornerRadius: 8).fill(Theme.fieldFill)
                        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Theme.cardStroke, lineWidth: 1)))
                HStack { Spacer()
                    Text("Save").font(.system(size: 12, weight: .medium)).foregroundStyle(Theme.textSecondary)
                        .padding(.horizontal, 14).frame(height: 28)
                        .background(RoundedRectangle(cornerRadius: 7).strokeBorder(Theme.cardStroke, lineWidth: 1))
                }
                .padding(.top, 10).padding(.bottom, 22)

                SettingsGroupLabel(text: "Memory (experimental)")
                SettingsRow(title: "Enable memories", subtitle: "Generate new memories from chats and bring them into new chats") { GlassToggle(on: true) }
                SettingsRow(title: "Chronicle research preview", subtitle: "Augment memories with screen context so CodeWith can help with anything you're working on") { GlassToggle(on: false) }
                SettingsRow(title: "Skip tool-assisted chats", subtitle: "Do not generate memories from chats that used MCP tools or web search") { GlassToggle(on: false) }
                SettingsRow(title: "Reset memories", subtitle: "Delete all of CodeWith memories", showDivider: false) {
                    Text("Reset").font(.system(size: 12, weight: .medium)).foregroundStyle(Theme.danger)
                        .padding(.horizontal, 14).frame(height: 28)
                        .background(RoundedRectangle(cornerRadius: 7).strokeBorder(Theme.danger.opacity(0.4), lineWidth: 1))
                }
            }
        }
    }
}
