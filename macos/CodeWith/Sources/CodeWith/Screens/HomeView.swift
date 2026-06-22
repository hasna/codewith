import SwiftUI

/// The "What should we work on?" empty state. Fully interactive: composer send,
/// "+", add-menu, and config pills all work.
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
                     onSubmit: onSubmit,
                     onPlus: { model.toggleAddMenu() },
                     onConfigTap: onToggleConfig,
                     modelLabel: model.model ?? "gpt-5.5",
                     effortLabel: model.effort)
                .frame(width: 392)

            HStack(spacing: 5) {
                Image(systemName: "folder").font(.system(size: 10))
                Text("Work in a project").font(.system(size: 11))
                Image(systemName: "chevron.down").font(.system(size: 8))
            }
            .foregroundStyle(Theme.textSecondary)
            .padding(.horizontal, 9).padding(.vertical, 5)
            .background(Capsule().fill(Theme.fieldFill).overlay(Capsule().strokeBorder(Theme.cardStroke, lineWidth: 1)))
            .padding(.top, 12)
            .frame(width: 392, alignment: .leading)

            Spacer()
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.canvas)
        .contentShape(Rectangle())
        .onTapGesture { if model.showAddMenu { model.showAddMenu = false } }
        .overlay(alignment: .center) {
            if model.showAddMenu {
                AddMenu(onAction: { model.handleAddAction($0) })
                    .offset(y: 40)
            }
        }
    }
}
