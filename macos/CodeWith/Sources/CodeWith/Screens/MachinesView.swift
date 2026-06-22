import SwiftUI

/// Fleet dashboard backed by the app-server machine registry.
struct MachinesView: View {
    var machines: [MachineInfo] = []

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
                if machines.isEmpty {
                    Text("No machines in your fleet yet.")
                        .font(.system(size: 12)).foregroundStyle(Theme.textTertiary)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 28).padding(.top, 20)
                } else {
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

    private func osLabel(_ os: String) -> String {
        let l = os.lowercased()
        if l.contains("mac") || l.contains("darwin") { return "macOS" }
        if l.contains("linux") { return "Linux" }
        if l.contains("ios") { return "iOS" }
        return os.capitalized
    }

    private func machineCard(_ m: MachineInfo) -> some View {
        let os = osLabel(m.os)
        return VStack(alignment: .leading, spacing: 0) {
            // Header: status dot + name + this-machine badge.
            HStack(spacing: 8) {
                Circle()
                    .fill(m.online ? Theme.success : Theme.textTertiary)
                    .frame(width: 8, height: 8)
                Text(m.id).font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.textPrimary).lineLimit(1)
                Spacer(minLength: 4)
                if m.isLocal {
                    Text("This machine")
                        .font(.system(size: 9.5, weight: .medium)).foregroundStyle(Theme.accent)
                        .padding(.horizontal, 6).padding(.vertical, 2)
                        .background(Capsule().fill(Theme.accent.opacity(0.10)))
                }
            }

            // OS + role subtitle.
            HStack(spacing: 5) {
                Image(systemName: os == "macOS" ? "apple.logo" : "terminal")
                    .font(.system(size: 10)).foregroundStyle(Theme.textSecondary)
                Text(m.role.isEmpty ? os : "\(os) · \(m.role)")
                    .font(.system(size: 11)).foregroundStyle(Theme.textSecondary).lineLimit(1)
            }
            .padding(.top, 8)

            Spacer(minLength: 14)

            // Stats footer.
            HStack(spacing: 0) {
                statTile(icon: m.online ? "wifi" : "wifi.slash",
                         label: m.online ? "online" : "offline",
                         color: m.online ? Theme.success : Theme.textTertiary)
                Rectangle().fill(Theme.separator).frame(width: 1, height: 24)
                statTile(icon: os == "macOS" ? "apple.logo" : "terminal", label: os, color: Theme.textSecondary)
                Rectangle().fill(Theme.separator).frame(width: 1, height: 24)
                statTile(icon: "circle.grid.cross", label: m.status, color: Theme.textSecondary)
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
