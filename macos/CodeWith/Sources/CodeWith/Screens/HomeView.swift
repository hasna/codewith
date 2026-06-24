import SwiftUI

/// The "What should we work on?" empty state. Composer send and config pills are
/// live controls.
struct HomeView: View {
    @Bindable var model: AppModel
    var onSubmit: () -> Void = {}
    var onToggleConfig: () -> Void = {}

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Spacer()
                Button(action: onToggleConfig) {
                    Image(systemName: "sidebar.right").font(.system(size: 13)).foregroundStyle(Theme.textTertiary)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
            .frame(height: 40).padding(.horizontal, 16)

            Spacer()

            Text("What should we work on?")
                .font(.system(size: 23, weight: .regular))
                .foregroundStyle(Theme.textPrimary)
                .padding(.bottom, 24)

            Composer(text: $model.composerText,
                     model: model,
                     onSubmit: onSubmit,
                     onPlus: { model.toggleAddMenu() },
                     onConfigTap: onToggleConfig,
                     modelLabel: model.model ?? "gpt-5.5",
                     effortLabel: model.effort)
                .frame(width: 480)

            Spacer()
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.canvas)
        .overlay {
            if model.showAddMenu {
                ZStack {
                    Color.black.opacity(0.001).contentShape(Rectangle())
                        .onTapGesture { model.showAddMenu = false }
                    AddMenu(
                        onAction: { model.handleAddAction($0) },
                        activePeers: model.activePeers,
                        agentRuns: model.addMenuAgentRuns
                    )
                    .offset(y: 40)
                }
            }
        }
    }
}
