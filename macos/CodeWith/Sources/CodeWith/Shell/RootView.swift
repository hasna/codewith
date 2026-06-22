import SwiftUI

/// Window content: sidebar + detail.
struct RootView<Detail: View>: View {
    var selected: String = ""
    @ViewBuilder var detail: () -> Detail

    var body: some View {
        HStack(spacing: 0) {
            Sidebar(selected: selected)
            Rectangle().fill(Theme.separator).frame(width: 1)
            detail()
        }
        .background(Theme.canvas)
    }
}
