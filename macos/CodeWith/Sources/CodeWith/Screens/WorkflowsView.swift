import SwiftUI

/// Workflow specs and runs across loaded sessions.
struct WorkflowsView: View {
    var workflows: [WorkflowInfo] = []
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
                        Text("Workflows unavailable: \(error)")
                            .font(.system(size: 12))
                            .foregroundStyle(Theme.textTertiary)
                            .padding(.top, 8)
                    } else if workflows.isEmpty {
                        Text("No workflows found across the loaded sessions.")
                            .font(.system(size: 12))
                            .foregroundStyle(Theme.textTertiary)
                            .padding(.top, 8)
                    } else {
                        ForEach(workflows) { workflow in
                            workflowRow(workflow)
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
            Text("Workflows").font(.system(size: 13)).foregroundStyle(Theme.textSecondary)
            Spacer()
        }
        .padding(.horizontal, 22)
        .frame(height: 40)
    }

    private func workflowRow(_ workflow: WorkflowInfo) -> some View {
        let thread = threads.first { $0.id == workflow.threadId }
        let icon = workflow.kind == .workflow ? "point.3.connected.trianglepath.dotted" : "play.circle"
        return Button {
            if let thread { onOpenThread(thread) }
        } label: {
            HStack(alignment: .top, spacing: 12) {
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .fill(Theme.success.opacity(0.14))
                    .frame(width: 32, height: 32)
                    .overlay(Image(systemName: icon).font(.system(size: 14)).foregroundStyle(Theme.success))
                VStack(alignment: .leading, spacing: 4) {
                    Text(workflow.title)
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(Theme.textPrimary)
                        .lineLimit(1)
                    Text(summary(workflow, thread: thread))
                        .font(.system(size: 11.5))
                        .foregroundStyle(Theme.textSecondary)
                        .lineLimit(1)
                }
                Spacer(minLength: 8)
                Text(workflow.status)
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

    private func summary(_ workflow: WorkflowInfo, thread: ThreadInfo?) -> String {
        let threadName = thread?.name ?? workflow.threadId
        return "\(threadName) - \(workflow.subtitle)"
    }
}
