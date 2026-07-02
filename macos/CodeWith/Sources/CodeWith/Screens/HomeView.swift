import SwiftUI

/// The "What should we work on?" empty state. Fully interactive: composer send,
/// "+", add-menu, and config pills all work.
struct HomeView: View {
    @Bindable var model: AppModel
    var showConfigToggle = true
    var onSubmit: () -> Void = {}
    var onToggleConfig: () -> Void = {}

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Spacer()
                if showConfigToggle {
                    Button(action: onToggleConfig) {
                        Image(systemName: "sidebar.right").font(.system(size: 13)).foregroundStyle(Theme.textTertiary)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                }
            }
            .frame(height: 40).padding(.horizontal, 16)

            Spacer()

            Text("What should we work on?")
                .font(.system(size: 23, weight: .regular))
                .foregroundStyle(Theme.textPrimary)
                .padding(.bottom, 24)

            Composer(text: $model.composerText,
                     onSubmit: onSubmit,
                     onPlus: { model.toggleAddMenu() },
                     onConfigTap: onToggleConfig,
                     modelLabel: model.model ?? "gpt-5.5",
                     effortLabel: model.effort)
                .frame(width: 480)

            HStack { ProjectMenu(model: model); Spacer() }
                .padding(.top, 12)
                .frame(width: 480, alignment: .leading)

            Spacer()
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.canvas)
        .overlay {
            // Dismiss layer + menu only while the add-menu is open, so it never
            // intercepts taps on the composer/send button when closed.
            if model.showAddMenu {
                ZStack {
                    Color.black.opacity(0.001).contentShape(Rectangle())
                        .onTapGesture { model.showAddMenu = false }
                    AddMenu(
                        onAction: { model.handleAddAction($0) },
                        activePeers: model.activePeers,
                        agentRuns: model.addMenuAgentRuns
                    ).offset(y: 40)
                }
            }
        }
    }
}
