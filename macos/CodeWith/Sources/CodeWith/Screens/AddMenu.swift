import SwiftUI

/// The "+" composer popover (reference screenshot 03).
struct AddMenu: View {
    var onAction: (String) -> Void = { _ in }
    var activePeers: [ActiveSessionPeerInfo] = []

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            label("Add")
            item(icon: "folder", title: "Files and folders", sub: nil, hover: true)
            item(icon: "terminal", title: "Attach Ghostty", sub: nil)
            item(icon: "target", title: "Goal", sub: "Set a goal that CodeWith will keep working towards")
            item(icon: "list.bullet.rectangle", title: "Plan mode", sub: "Turn plan mode on")
            label("Agents")
            if activePeers.isEmpty {
                disabledItem(title: "No active agents", sub: "Loaded agents and sessions appear here")
            } else {
                ForEach(activePeers) { peer in
                    item(icon: nil, title: peer.displayName, sub: peer.menuSubtitle, mono: true, actionValue: peer.peerId)
                }
            }
        }
        .padding(.vertical, 6)
        .frame(width: 380)
        .background(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .fill(Color.white)
                .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Theme.cardStroke, lineWidth: 1))
                .shadow(color: .black.opacity(0.16), radius: 24, y: 12)
        )
    }
    private func label(_ t: String) -> some View {
        Text(t).font(.system(size: 11, weight: .semibold)).foregroundStyle(Theme.textTertiary)
            .padding(.leading, 14).padding(.top, 8).padding(.bottom, 3)
    }
    private func item(icon: String?, title: String, sub: String?, hover: Bool = false, mono: Bool = false, actionValue: String? = nil) -> some View {
        Button { onAction(actionValue ?? title) } label: {
            HStack(spacing: 10) {
                if let icon {
                    Image(systemName: icon).font(.system(size: 12)).foregroundStyle(Theme.textSecondary).frame(width: 18)
                } else {
                    // Agents have no avatar in the reference — reserve the slot so
                    // names align with the labeled items above.
                    Color.clear.frame(width: 18, height: 18)
                }
                Text(title).font(.system(size: 12.5, weight: mono ? .semibold : .regular)).foregroundStyle(Theme.textPrimary).fixedSize()
                if let sub {
                    Text(sub).font(.system(size: 11.5)).foregroundStyle(Theme.textTertiary).lineLimit(1).truncationMode(.tail)
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 8).frame(height: 30)
            .contentShape(Rectangle())
            .background(RoundedRectangle(cornerRadius: 7).fill(hover ? Theme.rowSelected : .clear).padding(.horizontal, 6))
        }
        .buttonStyle(.plain)
    }

    private func disabledItem(title: String, sub: String?) -> some View {
        HStack(spacing: 10) {
            Color.clear.frame(width: 18, height: 18)
            Text(title).font(.system(size: 12.5, weight: .semibold)).foregroundStyle(Theme.textTertiary).fixedSize()
            if let sub {
                Text(sub).font(.system(size: 11.5)).foregroundStyle(Theme.textTertiary).lineLimit(1).truncationMode(.tail)
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 8).frame(height: 30)
    }
}
