import SwiftUI

/// Fleet dashboard backed by the app-server machine registry.
struct MachinesView: View {
    var machines: [MachineInfo] = []
    var error: String? = nil
    var pairing: MachinePairingInfo? = nil
    var onStartPairing: () -> Void = {}
    var onCheckPairing: () -> Void = {}
    @State private var showingAddMachine = false

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
                if let error {
                    Text("Machines unavailable: \(error)")
                        .font(.system(size: 12))
                        .foregroundStyle(Theme.textTertiary)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 28)
                        .padding(.top, 20)
                } else if machines.isEmpty {
                    Text("No machines in your fleet yet.")
                        .font(.system(size: 12)).foregroundStyle(Theme.textTertiary)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 28)
                        .padding(.top, 20)
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
        .sheet(isPresented: $showingAddMachine) {
            addMachineSheet
        }
    }

    private var addPill: some View {
        Button {
            showingAddMachine = true
            onStartPairing()
        } label: {
            HStack(spacing: 5) {
                Image(systemName: "plus").font(.system(size: 10, weight: .semibold))
                Text("Add machine").font(.system(size: 11.5, weight: .medium))
            }
            .foregroundStyle(.white)
            .padding(.horizontal, 12).frame(height: 26)
            .background(Capsule().fill(Color(hex: 0x202020)))
        }
        .buttonStyle(.plain)
    }

    private var addMachineSheet: some View {
        VStack(alignment: .leading, spacing: 16) {
            VStack(alignment: .leading, spacing: 4) {
                Text("Add machine")
                    .font(.system(size: 17, weight: .semibold))
                    .foregroundStyle(Theme.textPrimary)
                Text("Pair a machine through the CodeWith app-server.")
                    .font(.system(size: 11.5))
                    .foregroundStyle(Theme.textSecondary)
            }

            if let pairing {
                VStack(alignment: .leading, spacing: 8) {
                    Text("Pairing code")
                        .font(.system(size: 12))
                        .foregroundStyle(Theme.textSecondary)
                    Text(pairing.displayCode)
                        .font(.system(size: 20, weight: .semibold, design: .monospaced))
                        .foregroundStyle(Theme.textPrimary)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 12)
                        .frame(height: 42)
                        .background(
                            RoundedRectangle(cornerRadius: 7)
                                .fill(Theme.fieldFill)
                                .overlay(RoundedRectangle(cornerRadius: 7).strokeBorder(Theme.cardStroke, lineWidth: 1))
                        )
                    if pairing.expiresAt > 0 {
                        Text("Expires at \(pairing.expiresAt)")
                            .font(.system(size: 11))
                            .foregroundStyle(Theme.textTertiary)
                    }
                }
            } else {
                Text(error ?? "Requesting a pairing code...")
                    .font(.system(size: 12))
                    .foregroundStyle(Theme.textTertiary)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 12)
                    .frame(height: 42)
                    .background(
                        RoundedRectangle(cornerRadius: 7)
                            .fill(Theme.fieldFill)
                            .overlay(RoundedRectangle(cornerRadius: 7).strokeBorder(Theme.cardStroke, lineWidth: 1))
                    )
            }

            HStack {
                Spacer()
                Button("Cancel") { showingAddMachine = false }
                    .buttonStyle(.plain)
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(Theme.textSecondary)
                    .padding(.horizontal, 12)
                    .frame(height: 28)
                Button(action: onCheckPairing) {
                    Text("Check")
                        .font(.system(size: 12, weight: .medium))
                        .foregroundStyle(.white)
                        .padding(.horizontal, 14)
                        .frame(height: 28)
                        .background(Capsule().fill(Color(hex: 0x202020)))
                }
                .buttonStyle(.plain)
                .disabled(pairing == nil)
                .opacity(pairing == nil ? 0.45 : 1)
            }
        }
        .padding(20)
        .frame(width: 380)
        .background(Theme.canvas)
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
        let status = statusPresentation(m.status)
        return VStack(alignment: .leading, spacing: 0) {
            // Header: status dot + name + this-machine badge.
            HStack(spacing: 8) {
                Circle()
                    .fill(status.color)
                    .frame(width: 8, height: 8)
                Text(m.displayName).font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.textPrimary).lineLimit(1)
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
                statTile(icon: status.icon, label: status.label, color: status.color)
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

    private func statusPresentation(_ status: String) -> (icon: String, label: String, color: Color) {
        switch status.lowercased() {
        case "online":
            return ("wifi", "online", Theme.success)
        case "degraded":
            return ("wifi.exclamationmark", "degraded", Theme.warning)
        case "offline":
            return ("wifi.slash", "offline", Theme.textTertiary)
        default:
            return ("questionmark.circle", "unknown", Theme.textTertiary)
        }
    }

    private func statTile(icon: String, label: String, color: Color) -> some View {
        VStack(spacing: 3) {
            Image(systemName: icon).font(.system(size: 11)).foregroundStyle(color)
            Text(label).font(.system(size: 10)).foregroundStyle(color).lineLimit(1)
        }
        .frame(maxWidth: .infinity)
    }
}
