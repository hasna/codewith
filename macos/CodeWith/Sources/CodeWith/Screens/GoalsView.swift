import SwiftUI

/// Goals across loaded sessions, backed by thread/goal/list aggregation.
struct GoalsView: View {
    var states: [ThreadGoalState] = []
    var threads: [ThreadInfo] = []
    var error: String? = nil
    var onOpenThread: (ThreadInfo) -> Void = { _ in }

    var body: some View {
        VStack(spacing: 0) {
            topBar
            Rectangle().fill(Theme.separator).frame(height: 1)
            ScrollColumn(spacing: 0) {
                VStack(alignment: .leading, spacing: 8) {
                    if let error {
                        Text("Goals unavailable: \(error)")
                            .font(.system(size: 12))
                            .foregroundStyle(Theme.textTertiary)
                            .padding(.top, 8)
                    } else if states.isEmpty {
                        Text("No goals found across the loaded sessions.")
                            .font(.system(size: 12))
                            .foregroundStyle(Theme.textTertiary)
                            .padding(.top, 8)
                    } else {
                        ForEach(states) { state in
                            goalRow(state)
                        }
                    }
                }
                .padding(.horizontal, 24)
                .padding(.vertical, 20)
                .frame(maxWidth: 640, alignment: .leading)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.canvas)
    }

    private var topBar: some View {
        HStack {
            Text("Goals").font(.system(size: 13)).foregroundStyle(Theme.textSecondary)
            Spacer()
        }
        .padding(.horizontal, 22)
        .frame(height: 40)
    }

    private func goalRow(_ state: ThreadGoalState) -> some View {
        let thread = threads.first { $0.id == state.threadId }
        return Button {
            if let thread { onOpenThread(thread) }
        } label: {
            HStack(alignment: .top, spacing: 12) {
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .fill(Theme.accent.opacity(0.14))
                    .frame(width: 32, height: 32)
                    .overlay(Image(systemName: "target").font(.system(size: 14)).foregroundStyle(Theme.accent))
                VStack(alignment: .leading, spacing: 4) {
                    Text(goalTitle(state))
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(Theme.textPrimary)
                        .lineLimit(1)
                    Text(summary(state, thread: thread))
                        .font(.system(size: 11.5))
                        .foregroundStyle(Theme.textSecondary)
                        .lineLimit(1)
                }
                Spacer(minLength: 8)
                Text(state.goal?.status ?? state.goalPlans.first?.status ?? "active")
                    .font(.system(size: 11))
                    .foregroundStyle(Theme.textTertiary)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 11)
            .contentShape(Rectangle())
            .background(
                RoundedRectangle(cornerRadius: Theme.cardRadius, style: .continuous)
                    .fill(Theme.fieldFill)
                    .overlay(RoundedRectangle(cornerRadius: Theme.cardRadius, style: .continuous).strokeBorder(Theme.cardStroke, lineWidth: 1))
            )
        }
        .buttonStyle(.plain)
        .disabled(thread == nil)
    }

    private func summary(_ state: ThreadGoalState, thread: ThreadInfo?) -> String {
        let planCount = state.goalPlans.count
        let threadName = thread?.name ?? state.threadId
        if planCount == 0 { return threadName }
        return "\(threadName) - \(planCount) goal plan\(planCount == 1 ? "" : "s")"
    }

    private func goalTitle(_ state: ThreadGoalState) -> String {
        guard let objective = state.goal?.objective, !objective.isEmpty else {
            return "Goal plan"
        }
        return objective
    }
}
