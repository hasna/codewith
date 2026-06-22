import SwiftUI

/// The "+" composer popover (reference screenshot 03).
struct AddMenu: View {
    var onAction: (String) -> Void = { _ in }

    private let agents: [(String, String)] = [
        ("Apollo", "UX designer — interfaces, UX, accessibility"),
        ("Ares", "Performance engineer — profiling, optimization, speed"),
        ("Athena", "Product manager — requirements, prioritization, user stories"),
        ("Atlas", "Mobile engineer — iOS, Android, cross-platform"),
        ("Aurelius", "Refactoring expert — code quality, clean architecture, technical debt"),
    ]
    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            label("Add")
            item(icon: "folder", title: "Files and folders", sub: nil, hover: true)
            item(icon: "terminal", title: "Attach Ghostty", sub: nil)
            item(icon: "target", title: "Goal", sub: "Set a goal that CodeWith will keep working towards")
            item(icon: "list.bullet.rectangle", title: "Plan mode", sub: "Turn plan mode on")
            label("Agents")
            ForEach(agents, id: \.0) { a in
                item(icon: nil, title: a.0, sub: a.1, mono: true)
            }
        }
        .padding(.vertical, 6)
        .frame(width: 412)
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
    private func item(icon: String?, title: String, sub: String?, hover: Bool = false, mono: Bool = false) -> some View {
        Button { onAction(title) } label: {
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
}
