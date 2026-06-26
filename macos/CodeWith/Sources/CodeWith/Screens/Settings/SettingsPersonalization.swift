import SwiftUI

struct SettingsPersonalization: View {
    var instructions: String = ""
    var desktopSettings = DesktopSettingsInfo()
    var onSetPersonality: (String) -> Void = { _ in }
    var onSaveInstructions: (String) -> Void = { _ in }
    var onSetMemoryEnabled: (Bool) -> Void = { _ in }
    var onSetChronicleResearch: (Bool) -> Void = { _ in }
    var onSetSkipToolAssistedChats: (Bool) -> Void = { _ in }
    var onResetMemories: () -> Void = {}
    @State private var draftInstructions = ""

    var body: some View {
        SettingsPage(title: "Personalization") {
            VStack(alignment: .leading, spacing: 0) {
                SettingsRow(title: "Personality", subtitle: "Choose a default tone for CodeWith responses", showDivider: false) {
                    Menu {
                        Button("Pragmatic") { onSetPersonality("pragmatic") }
                        Button("Friendly") { onSetPersonality("friendly") }
                        Button("None") { onSetPersonality("none") }
                    } label: {
                        DropdownPill(text: personalityLabel)
                    }
                    .menuStyle(.borderlessButton)
                    .menuIndicator(.hidden)
                    .fixedSize()
                }
                .padding(.bottom, 18)

                HStack(spacing: 4) {
                    Text("Custom instructions").font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.textPrimary)
                    Spacer()
                }
                Text("Give CodeWith extra instructions and context for all tasks on this host.")
                    .font(.system(size: 11.5)).foregroundStyle(Theme.textSecondary).padding(.top, 4).padding(.bottom, 8)
                TextEditor(text: $draftInstructions)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(Theme.textPrimary)
                    .scrollContentBackground(.hidden)
                    .padding(8)
                    .frame(height: 156, alignment: .topLeading)
                    .background(RoundedRectangle(cornerRadius: 8).fill(Theme.fieldFill)
                        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Theme.cardStroke, lineWidth: 1)))
                HStack { Spacer()
                    Button {
                        onSaveInstructions(draftInstructions)
                    } label: {
                        Text("Save").font(.system(size: 12, weight: .medium)).foregroundStyle(Theme.textSecondary)
                            .padding(.horizontal, 14).frame(height: 28)
                            .background(RoundedRectangle(cornerRadius: 7).strokeBorder(Theme.cardStroke, lineWidth: 1))
                    }
                    .buttonStyle(.plain)
                }
                .padding(.top, 10).padding(.bottom, 22)

                SettingsGroupLabel(text: "Memory (experimental)")
                SettingsRow(title: "Enable memories", subtitle: "Generate new memories from chats and bring them into new chats") {
                    GlassToggle(on: desktopSettings.memoryEnabled) { onSetMemoryEnabled(!desktopSettings.memoryEnabled) }
                }
                SettingsRow(title: "Chronicle research preview", subtitle: "Augment memories with screen context so CodeWith can help with anything you're working on") {
                    GlassToggle(on: desktopSettings.chronicleResearch) { onSetChronicleResearch(!desktopSettings.chronicleResearch) }
                }
                SettingsRow(title: "Skip tool-assisted chats", subtitle: "Do not generate memories from chats that used MCP tools or web search") {
                    GlassToggle(on: desktopSettings.skipToolAssistedChats) { onSetSkipToolAssistedChats(!desktopSettings.skipToolAssistedChats) }
                }
                SettingsRow(title: "Reset memories", subtitle: "Delete all of CodeWith memories", showDivider: false) {
                    Button(action: onResetMemories) {
                        Text("Reset").font(.system(size: 12, weight: .medium)).foregroundStyle(Theme.danger)
                            .padding(.horizontal, 14).frame(height: 28)
                            .background(RoundedRectangle(cornerRadius: 7).strokeBorder(Theme.danger.opacity(0.4), lineWidth: 1))
                    }
                    .buttonStyle(.plain)
                }
            }
        }
        .onAppear { draftInstructions = instructions }
        .onChange(of: instructions) { _, newValue in draftInstructions = newValue }
    }

    private var personalityLabel: String {
        switch desktopSettings.personality {
        case "friendly": return "Friendly"
        case "none": return "None"
        default: return "Pragmatic"
        }
    }
}
