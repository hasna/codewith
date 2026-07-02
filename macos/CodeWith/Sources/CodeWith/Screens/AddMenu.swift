import SwiftUI

enum AddMenuAction: Equatable {
    case filesAndFolders
    case attachGhostty
    case goal
    case planMode
    case activePeer(String)
    case agentRun(String)
}

/// Legacy add-action popover retained for non-composer callers.
struct AddMenu: View {
    var onAction: (AddMenuAction) -> Void = { _ in }
    var activePeers: [ActiveSessionPeerInfo] = []
    var agentRuns: [AgentRunInfo] = []

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            label("Add")
            item(icon: "folder", title: "Open folder as project", sub: nil, hover: true, action: .filesAndFolders)
            disabledItem(title: "Attach Ghostty", sub: "Unavailable")
            item(icon: "target", title: "Goal", sub: "Set a goal that CodeWith will keep working towards", action: .goal)
            item(icon: "list.bullet.rectangle", title: "Plan mode", sub: "Turn plan mode on", action: .planMode)
            label("Agents")
            if activePeers.isEmpty && agentRuns.isEmpty {
                disabledItem(title: "No active agents", sub: "Loaded agents and sessions appear here")
            } else {
                ForEach(activePeers) { peer in
                    item(icon: nil, title: peer.displayName, sub: peerSubtitle(peer), mono: true, action: .activePeer(peer.peerId))
                }
                ForEach(agentRuns) { agent in
                    item(icon: nil, title: agent.displayName, sub: agentSubtitle(agent), mono: true, action: .agentRun(agent.agentId))
                }
            }
        }
        .padding(.vertical, 6)
        .frame(width: 380)
        .background(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .fill(Theme.fieldFill)
                .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Theme.cardStroke, lineWidth: 1))
                .shadow(color: .black.opacity(0.16), radius: 24, y: 12)
        )
    }
    private func label(_ t: String) -> some View {
        Text(t).font(.system(size: 11, weight: .semibold)).foregroundStyle(Theme.textTertiary)
            .padding(.leading, 14).padding(.top, 8).padding(.bottom, 3)
    }
    private func item(icon: String?, title: String, sub: String?, hover: Bool = false, mono: Bool = false, action: AddMenuAction) -> some View {
        Button { onAction(action) } label: {
            HStack(spacing: 10) {
                if let icon {
                    Image(systemName: icon).font(.system(size: 12)).foregroundStyle(Theme.textSecondary).frame(width: 18)
                } else {
                    // Agents have no avatar in the reference; reserve the slot so
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

    private func peerSubtitle(_ peer: ActiveSessionPeerInfo) -> String {
        guard activePeers.filter({ $0.displayName == peer.displayName }).count > 1 else {
            return peer.menuSubtitle
        }
        return "\(peer.menuSubtitle) · \(shortId(peer.threadId.isEmpty ? peer.peerId : peer.threadId))"
    }

    private func agentSubtitle(_ agent: AgentRunInfo) -> String {
        guard agentRuns.filter({ $0.displayName == agent.displayName }).count > 1 else {
            return agent.menuSubtitle
        }
        return "\(agent.menuSubtitle) · \(shortId(agent.agentId))"
    }

    private func shortId(_ id: String) -> String {
        String(id.prefix(8))
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
