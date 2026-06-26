import SwiftUI

/// Loops — recurring schedules + monitors running across all sessions (live data).
struct LoopsView: View {
    var loops: [LoopInfo] = []
    var onToggle: (LoopInfo) -> Void = { _ in }
    var onCreate: () -> Void = {}

    var body: some View {
        VStack(spacing: 0) {
            topBar
            Rectangle().fill(Theme.separator).frame(height: 1)
            ScrollColumn(spacing: 0) {
                VStack(alignment: .leading, spacing: 8) {
                    if loops.isEmpty {
                        Text("No loops running across your sessions yet.")
                            .font(.system(size: 12)).foregroundStyle(Theme.textTertiary)
                            .padding(.top, 8)
                    } else {
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
            Text("Loops").font(.system(size: 13)).foregroundStyle(Theme.textSecondary)
            Spacer()
            Button(action: onCreate) {
                HStack(spacing: 5) {
                    Image(systemName: "plus").font(.system(size: 10, weight: .semibold))
                    Text("New loop").font(.system(size: 11.5, weight: .medium))
                }
                .foregroundStyle(.white)
                .padding(.horizontal, 12).frame(height: 26)
                .background(Capsule().fill(Theme.accent))
            }
            .buttonStyle(.plain)
        }
        .padding(.horizontal, 22).frame(height: 40)
    }

    private func loopRow(_ loop: LoopInfo) -> some View {
        let isSchedule = loop.kind == .schedule
        let tint = isSchedule ? Theme.accent : Theme.success
        let icon = isSchedule ? "clock.arrow.circlepath" : "dot.radiowaves.left.and.right"
        return HStack(alignment: .center, spacing: 12) {
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .fill(tint.opacity(0.14))
                .frame(width: 32, height: 32)
                .overlay(Image(systemName: icon).font(.system(size: 14)).foregroundStyle(tint))
            VStack(alignment: .leading, spacing: 2) {
                Text(loop.title).font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.textPrimary).lineLimit(1)
                Text(loop.subtitle).font(.system(size: 11.5)).foregroundStyle(Theme.textSecondary).lineLimit(1)
            }
            Spacer()
            GlassToggle(on: loop.active) { onToggle(loop) }
        }
        .padding(.horizontal, 12).padding(.vertical, 11)
        .background(
            RoundedRectangle(cornerRadius: Theme.cardRadius, style: .continuous)
                .fill(Theme.fieldFill)
                .overlay(RoundedRectangle(cornerRadius: Theme.cardRadius, style: .continuous).strokeBorder(Theme.cardStroke, lineWidth: 1))
        )
    }
}
