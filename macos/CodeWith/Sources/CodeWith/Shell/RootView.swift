import SwiftUI

/// Window content for snapshots/previews: sidebar + detail driven by a model.
struct RootView<Detail: View>: View {
    var model: AppModel
    @ViewBuilder var detail: () -> Detail

    var body: some View {
        HStack(spacing: 0) {
            Sidebar(model: model)
            Rectangle().fill(Theme.separator).frame(width: 1)
            detail()
        }
        .background(Theme.canvas)
    }
}
