import SwiftUI

/// "Loops & Goals" — active goals the agent works toward plus recurring
/// schedules (loops). Goals show a progress bar; loops show an on/off toggle.
struct LoopsView: View {
    private struct Goal: Identifiable {
        let id = UUID()
        let icon: String
        let tint: Color
        let title: String
        let status: String
        let progress: Double?   // nil == queued
    }

    private struct Loop: Identifiable {
        let id = UUID()
        let icon: String
        let tint: Color
        let title: String
        let cadence: String
        let on: Bool
    }

    private let goals: [Goal] = [
        Goal(icon: "hammer.fill", tint: Color(hex: 0x5856D6),
             title: "Build CodeWith macOS app", status: "in progress 78%", progress: 0.78),
        Goal(icon: "shippingbox.fill", tint: Color(hex: 0xFF9500),
             title: "Ship parity release", status: "queued", progress: nil),
    ]

    private let loops: [Loop] = [
        Loop(icon: "sun.max.fill", tint: Color(hex: 0xFFCC00),
             title: "Daily standup", cadence: "every day · 9:00", on: true),
        Loop(icon: "arrow.triangle.branch", tint: Color(hex: 0x34C759),
             title: "PR babysitter", cadence: "every 5m", on: true),
        Loop(icon: "checkmark.shield.fill", tint: Color(hex: 0xFF3B30),
             title: "Security sweep", cadence: "weekly", on: false),
    ]

    var body: some View {
        VStack(spacing: 0) {
            topBar
            ScrollColumn(spacing: 0) {
                VStack(alignment: .leading, spacing: 18) {
                    section(label: "Goals") {
                        ForEach(goals) { goal in goalRow(goal) }
                    }
                    section(label: "Loops") {
                        ForEach(loops) { loop in loopRow(loop) }
                    }
                }
                .padding(.horizontal, 24)
                .padding(.vertical, 20)
                .frame(maxWidth: 560, alignment: .leading)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.canvas)
    }

    private var topBar: some View {
        HStack {
            Text("Loops")
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(Theme.textPrimary)
            Spacer()
            HStack(spacing: 5) {
                Image(systemName: "plus").font(.system(size: 10, weight: .semibold))
                Text("New loop").font(.system(size: 11.5, weight: .medium))
            }
            .foregroundStyle(.white)
            .padding(.horizontal, 12).frame(height: 26)
            .background(Capsule().fill(Theme.accent))
        }
        .frame(height: 38)
        .padding(.horizontal, 16)
        .overlay(alignment: .bottom) { Rectangle().fill(Theme.separator).frame(height: 1) }
    }

    private func section<Content: View>(label: String, @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(label.uppercased())
                .font(.system(size: 11, weight: .semibold))
                .tracking(0.4)
                .foregroundStyle(Theme.textTertiary)
            VStack(spacing: 8) { content() }
        }
    }

    private func goalRow(_ goal: Goal) -> some View {
        row(icon: goal.icon, tint: goal.tint) {
            VStack(alignment: .leading, spacing: 6) {
                HStack(alignment: .firstTextBaseline) {
                    Text(goal.title).font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.textPrimary)
                    Spacer()
                    Text(goal.status).font(.system(size: 11)).foregroundStyle(Theme.textSecondary)
                }
                if let progress = goal.progress {
                    GeometryReader { geo in
                        ZStack(alignment: .leading) {
                            Capsule().fill(Theme.fieldFill).overlay(Capsule().strokeBorder(Theme.cardStroke, lineWidth: 1))
                            Capsule().fill(goal.tint).frame(width: geo.size.width * progress)
                        }
                    }
                    .frame(height: 6)
                } else {
                    Capsule().fill(Theme.fieldFill).overlay(Capsule().strokeBorder(Theme.cardStroke, lineWidth: 1)).frame(height: 6)
                }
            }
        }
    }

    private func loopRow(_ loop: Loop) -> some View {
        row(icon: loop.icon, tint: loop.tint) {
            HStack(spacing: 10) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(loop.title).font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.textPrimary)
                    Text(loop.cadence).font(.system(size: 11.5)).foregroundStyle(Theme.textSecondary)
                }
                Spacer()
                GlassToggle(on: loop.on)
            }
        }
    }

    private func row<Content: View>(icon: String, tint: Color, @ViewBuilder content: () -> Content) -> some View {
        HStack(alignment: .center, spacing: 12) {
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .fill(tint.opacity(0.14))
                .frame(width: 32, height: 32)
                .overlay(Image(systemName: icon).font(.system(size: 14)).foregroundStyle(tint))
            content()
        }
        .padding(.horizontal, 12).padding(.vertical, 11)
        .background(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .fill(Theme.fieldFill)
                .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(Theme.cardStroke, lineWidth: 1))
        )
    }
}
