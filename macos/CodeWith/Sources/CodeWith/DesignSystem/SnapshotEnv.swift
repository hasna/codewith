import SwiftUI

/// True while rendering via `ImageRenderer` (snapshot mode). `ScrollView`
/// content does not render offscreen under `ImageRenderer`, so scrollable
/// regions fall back to a plain stack when this is set.
private struct SnapshotModeKey: EnvironmentKey {
    static let defaultValue = false
}

extension EnvironmentValues {
    var snapshotMode: Bool {
        get { self[SnapshotModeKey.self] }
        set { self[SnapshotModeKey.self] = newValue }
    }
}

/// A vertical scroll region that degrades to a plain `VStack` while snapshotting.
struct ScrollColumn<Content: View>: View {
    var alignment: HorizontalAlignment = .leading
    var spacing: CGFloat = 0
    @Environment(\.snapshotMode) private var snapshot
    @ViewBuilder var content: () -> Content

    var body: some View {
        if snapshot {
            VStack(alignment: alignment, spacing: spacing, content: content)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        } else {
            ScrollView {
                VStack(alignment: alignment, spacing: spacing, content: content)
            }
        }
    }
}
