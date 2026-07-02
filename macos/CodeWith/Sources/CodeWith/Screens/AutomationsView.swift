import SwiftUI

/// "Automations" — trigger-based rules the agent runs automatically. Each row
/// shows the rule name, its trigger, last-run time, and an on/off toggle.
struct AutomationsView: View {
    private struct Automation: Identifiable {
        let id = UUID()
        let icon: String
        let tint: Color
        let name: String
        let trigger: String
        let lastRun: String
        let on: Bool
    }

    private let automations: [Automation] = [
        Automation(icon: "checkmark.seal.fill", tint: Color(hex: 0x5856D6),
                   name: "Auto-review PRs", trigger: "When PR opened", lastRun: "12m ago", on: true),
        Automation(icon: "sun.horizon.fill", tint: Color(hex: 0xFF9500),
                   name: "Morning briefing", trigger: "Every morning · 8:00", lastRun: "Today 8:00", on: true),
        Automation(icon: "arrow.up.forward.app.fill", tint: Color(hex: 0x34C759),
                   name: "Deploy on green", trigger: "On push to main", lastRun: "1h ago", on: false),
        Automation(icon: "tray.full.fill", tint: Color(hex: 0xFF3B30),
                   name: "Triage new issues", trigger: "When issue created", lastRun: "Yesterday", on: true),
    ]

    var body: some View {
        VStack(spacing: 0) {
            topBar
            ScrollColumn(spacing: 0) {
                VStack(spacing: 8) {
                    ForEach(automations) { automation in row(automation) }
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
            Text("Automations")
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(Theme.textPrimary)
            Spacer()
            HStack(spacing: 5) {
                Image(systemName: "plus").font(.system(size: 10, weight: .semibold))
                Text("New automation").font(.system(size: 11.5, weight: .medium))
            }
            .foregroundStyle(.white)
            .padding(.horizontal, 12).frame(height: 26)
            .background(Capsule().fill(Theme.accent))
        }
        .frame(height: 38)
        .padding(.horizontal, 16)
        .overlay(alignment: .bottom) { Rectangle().fill(Theme.separator).frame(height: 1) }
    }

    private func row(_ automation: Automation) -> some View {
        HStack(alignment: .center, spacing: 12) {
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .fill(automation.tint.opacity(0.14))
                .frame(width: 32, height: 32)
                .overlay(Image(systemName: automation.icon).font(.system(size: 14)).foregroundStyle(automation.tint))
            VStack(alignment: .leading, spacing: 2) {
                Text(automation.name).font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.textPrimary)
                Text(automation.trigger).font(.system(size: 11.5)).foregroundStyle(Theme.textSecondary)
            }
            Spacer()
            Text(automation.lastRun).font(.system(size: 11)).foregroundStyle(Theme.textTertiary)
            GlassToggle(on: automation.on)
        }
        .padding(.horizontal, 12).padding(.vertical, 11)
        .background(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .fill(Theme.fieldFill)
                .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(Theme.cardStroke, lineWidth: 1))
        )
    }
}
