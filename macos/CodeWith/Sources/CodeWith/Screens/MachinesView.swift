import SwiftUI

/// Fleet dashboard — a fork-specific screen listing the machines CodeWith can
/// run agents on. Self-contained sample data, no AppModel dependency.
struct MachinesView: View {
    private struct Machine: Identifiable {
        let id = UUID()
        let name: String
        let os: String          // SF Symbol-friendly label
        let role: String
        let online: Bool
        let thisMachine: Bool
        let tailscale: Bool
        let cpu: String         // e.g. "8% CPU"
        let lastSeen: String    // e.g. "now", "3m ago", "2d ago"
    }

    private let machines: [Machine] = [
        Machine(name: "spark01", os: "Linux", role: "Primary dev box", online: true, thisMachine: true, tailscale: true, cpu: "12% CPU", lastSeen: "now"),
        Machine(name: "spark02", os: "Linux", role: "Secondary server", online: true, thisMachine: false, tailscale: true, cpu: "4% CPU", lastSeen: "now"),
        Machine(name: "apple03", os: "macOS", role: "Workstation", online: true, thisMachine: false, tailscale: true, cpu: "9% CPU", lastSeen: "now"),
        Machine(name: "machine001", os: "macOS", role: "Build runner", online: true, thisMachine: false, tailscale: true, cpu: "21% CPU", lastSeen: "now"),
        Machine(name: "apple06", os: "macOS", role: "Laptop", online: false, thisMachine: false, tailscale: true, cpu: "—", lastSeen: "3m ago"),
        Machine(name: "machine002", os: "Linux", role: "CI worker", online: false, thisMachine: false, tailscale: true, cpu: "—", lastSeen: "2h ago"),
        Machine(name: "machine003", os: "macOS", role: "Test box", online: false, thisMachine: false, tailscale: false, cpu: "—", lastSeen: "1d ago"),
        Machine(name: "machine004", os: "Linux", role: "Spare", online: false, thisMachine: false, tailscale: false, cpu: "—", lastSeen: "5d ago"),
    ]

    private let columns = [
        GridItem(.flexible(), spacing: 14),
        GridItem(.flexible(), spacing: 14),
        GridItem(.flexible(), spacing: 14),
    ]

    var body: some View {
        VStack(spacing: 0) {
            // Detail top bar.
            HStack {
                Text("Machines").font(.system(size: 13)).foregroundStyle(Theme.textSecondary)
                Spacer()
                addPill
            }
            .padding(.horizontal, 22).frame(height: 40)
            Rectangle().fill(Theme.separator).frame(height: 1)

            ScrollColumn(spacing: 0) {
                // Summary line.
                HStack(spacing: 6) {
                    Text("\(machines.filter(\.online).count) online")
                        .font(.system(size: 12, weight: .medium)).foregroundStyle(Theme.textPrimary)
                    Text("·").foregroundStyle(Theme.textTertiary)
                    Text("\(machines.count) machines in fleet")
                        .font(.system(size: 12)).foregroundStyle(Theme.textSecondary)
                    Spacer()
                }
                .padding(.horizontal, 28).padding(.top, 20).padding(.bottom, 16)

                LazyVGrid(columns: columns, spacing: 14) {
                    ForEach(machines) { machineCard($0) }
                }
                .padding(.horizontal, 28)
                .padding(.bottom, 28)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.canvas)
    }

    private var addPill: some View {
        HStack(spacing: 5) {
            Image(systemName: "plus").font(.system(size: 10, weight: .semibold))
            Text("Add machine").font(.system(size: 11.5, weight: .medium))
        }
        .foregroundStyle(.white)
        .padding(.horizontal, 12).frame(height: 26)
        .background(Capsule().fill(Color(hex: 0x202020)))
    }

    private func machineCard(_ m: Machine) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            // Header: status dot + name + this-machine badge.
            HStack(spacing: 8) {
                Circle()
                    .fill(m.online ? Theme.success : Color(hex: 0xC4C4C8))
                    .frame(width: 8, height: 8)
                Text(m.name).font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.textPrimary)
                Spacer(minLength: 4)
                if m.thisMachine {
                    Text("This machine")
                        .font(.system(size: 9.5, weight: .medium)).foregroundStyle(Theme.accent)
                        .padding(.horizontal, 6).padding(.vertical, 2)
                        .background(Capsule().fill(Theme.accent.opacity(0.10)))
                }
            }

            // OS + role subtitle.
            HStack(spacing: 5) {
                Image(systemName: m.os == "macOS" ? "apple.logo" : "terminal")
                    .font(.system(size: 10)).foregroundStyle(Theme.textSecondary)
                Text("\(m.os) · \(m.role)")
                    .font(.system(size: 11)).foregroundStyle(Theme.textSecondary)
            }
            .padding(.top, 8)

            Spacer(minLength: 14)

            // Stats footer.
            HStack(spacing: 0) {
                statTile(icon: "network",
                         label: m.tailscale ? "tailscale" : "offline",
                         color: m.tailscale ? Theme.textSecondary : Theme.textTertiary)
                Rectangle().fill(Theme.separator).frame(width: 1, height: 24)
                statTile(icon: "cpu", label: m.online ? m.cpu : "idle", color: Theme.textSecondary)
                Rectangle().fill(Theme.separator).frame(width: 1, height: 24)
                statTile(icon: "clock", label: m.lastSeen, color: Theme.textSecondary)
            }
            .padding(.top, 4)
        }
        .padding(14)
        .frame(height: 132, alignment: .topLeading)
        .background(
            RoundedRectangle(cornerRadius: Theme.cardRadius, style: .continuous)
                .fill(Theme.fieldFill)
                .overlay(
                    RoundedRectangle(cornerRadius: Theme.cardRadius, style: .continuous)
                        .strokeBorder(Theme.cardStroke, lineWidth: 1)
                )
        )
    }

    private func statTile(icon: String, label: String, color: Color) -> some View {
        VStack(spacing: 3) {
            Image(systemName: icon).font(.system(size: 11)).foregroundStyle(color)
            Text(label).font(.system(size: 10)).foregroundStyle(color).lineLimit(1)
        }
        .frame(maxWidth: .infinity)
    }
}
