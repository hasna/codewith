import SwiftUI

/// A live session view, driven by `model.activeMessages`.
struct ChatView: View {
    @Bindable var model: AppModel
    var threadId: String
    var onSubmit: () -> Void = {}
    var onPlus: () -> Void = {}
    var onAddAction: (AddMenuAction) -> Void = { _ in }
    var showConfigToggle = true
    var onToggleConfig: () -> Void = {}

    private var title: String {
        model.threads.first { $0.id == threadId }?.name ?? "Chat"
    }

    var body: some View {
        VStack(spacing: 0) {
            // Top bar with the session title and session-details opener.
            HStack(spacing: 8) {
                Text(title).font(.system(size: 13, weight: .medium)).foregroundStyle(Theme.textPrimary).lineLimit(1)
                Image(systemName: "ellipsis").font(.system(size: 12)).foregroundStyle(Theme.textTertiary)
                Spacer()
                if showConfigToggle {
                    Button(action: onToggleConfig) {
                        Image(systemName: "sidebar.right").font(.system(size: 13)).foregroundStyle(Theme.textTertiary)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(.horizontal, 16).frame(height: 40)
            Rectangle().fill(Theme.separator).frame(height: 1)

            // Conversation
            ScrollColumn(alignment: .leading, spacing: 0) {
                if model.activeMessages.isEmpty {
                    Text(model.visibleTurnInProgress ? "Working…" : "")
                        .font(.system(size: 12)).foregroundStyle(Theme.textTertiary)
                        .padding(.top, 8)
                } else {
                    ForEach(model.activeMessages) { messageView($0) }
                    if model.visibleTurnInProgress {
                        Text("Working…").font(.system(size: 12)).foregroundStyle(Theme.textTertiary).padding(.top, 4)
                    }
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 24).padding(.top, 18)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)

            if let pending = model.pendingServerRequestForActiveThread {
                PendingServerRequestPanel(
                    prompt: pending,
                    onAction: { action in model.respondToServerRequest(pending, action: action) }
                )
                .id(pending.id)
                .padding(.horizontal, 24)
                .padding(.top, 6)
            } else if let pending = model.pendingUserInputForActiveThread {
                PendingUserInputPanel(
                    prompt: pending,
                    onSubmit: { answers in model.respondToUserInputRequest(pending, answers: answers) },
                    onCancel: { model.cancelUserInputRequest(pending) }
                )
                .id(pending.id)
                .padding(.horizontal, 24)
                .padding(.top, 6)
            } else if let pending = model.pendingMcpElicitationForActiveThread {
                PendingMcpElicitationPanel(
                    prompt: pending,
                    onOpenURL: { model.openMcpElicitationURL(pending) },
                    onAccept: { content in model.respondToMcpElicitationRequest(pending, action: "accept", content: content) },
                    onDecline: { model.respondToMcpElicitationRequest(pending, action: "decline") },
                    onCancel: { model.respondToMcpElicitationRequest(pending, action: "cancel") }
                )
                .id(pending.id)
                .padding(.horizontal, 24)
                .padding(.top, 6)
            }

            if let attachment = model.activeAgentAttachment {
                AgentAttachmentPanel(attachment: attachment)
                    .padding(.horizontal, 24)
                    .padding(.top, 6)
            }

            if !model.activeGoalPlans.isEmpty {
                GoalPlanPanel(
                    goal: model.activeGoal,
                    plans: model.activeGoalPlans,
                    onActivate: { node in Task { await model.activateGoalPlanNode(node) } }
                )
                .padding(.horizontal, 24)
                .padding(.top, 6)
            }

            // Composer
            Composer(placeholder: "Ask for follow-up changes",
                     stopMode: model.visibleTurnInProgress,
                     text: $model.composerText, model: model, onSubmit: onSubmit,
                     onStop: { Task { await model.interrupt() } },
                     onPlus: { model.toggleAddMenu() },
                     onConfigTap: onToggleConfig,
                     modelLabel: model.model ?? "gpt-5.5", effortLabel: model.effort)
                .padding(.horizontal, 24).padding(.vertical, 14)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.canvas)
        .overlay(alignment: .bottomLeading) {
            if model.showAddMenu {
                AddMenu(
                    onAction: { model.handleAddAction($0) },
                    activePeers: model.activePeers,
                    agentRuns: model.addMenuAgentRuns
                )
                .padding(.leading, 24)
                .padding(.bottom, 68)
            }
        }
    }

    @ViewBuilder
    private func messageView(_ m: ChatMessage) -> some View {
        switch m.role {
        case .user:
            HStack { Spacer()
                Text(m.text).font(.system(size: 13)).foregroundStyle(Theme.textPrimary)
                    .padding(.horizontal, 12).padding(.vertical, 7)
                    .background(RoundedRectangle(cornerRadius: 14).fill(Color(hex: 0xEDEDEF)))
            }
            .padding(.bottom, 16)
        case .assistant:
            Text(m.text).font(.system(size: 13)).foregroundStyle(Theme.textPrimary)
                .fixedSize(horizontal: false, vertical: true).lineSpacing(3)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.bottom, 12)
        case .tool:
            ToolRow(icon: m.toolIcon ?? "wrench.and.screwdriver", text: m.text)
        }
    }
}

struct AgentAttachmentPanel: View {
    var attachment: AgentAttachmentInfo

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: "person.crop.circle.badge.checkmark")
                .font(.system(size: 13))
                .foregroundStyle(Theme.success)
                .frame(width: 18)
                .padding(.top, 2)
            VStack(alignment: .leading, spacing: 6) {
                Text(title)
                    .font(.system(size: 12.5, weight: .semibold))
                    .foregroundStyle(Theme.textPrimary)
                    .lineLimit(1)
                Text(subtitle)
                    .font(.system(size: 11.5))
                    .foregroundStyle(Theme.textSecondary)
                    .lineLimit(2)
                ForEach(attachment.pendingInteractions.prefix(3)) { interaction in
                    HStack(spacing: 6) {
                        Circle()
                            .fill(Theme.warning)
                            .frame(width: 6, height: 6)
                        Text(interaction.summary)
                            .font(.system(size: 11))
                            .foregroundStyle(Theme.textSecondary)
                            .lineLimit(1)
                        Spacer(minLength: 4)
                        Text(interaction.status)
                            .font(.system(size: 11))
                            .foregroundStyle(Theme.textTertiary)
                    }
                }
            }
            Spacer(minLength: 8)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .fill(Theme.fieldFill)
                .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(Theme.cardStroke, lineWidth: 1))
        )
    }

    private var title: String {
        guard let agent = attachment.agent else { return "Agent attached" }
        return "\(agent.displayName) attached"
    }

    private var subtitle: String {
        var parts = [attachment.status]
        if attachment.eventCount > 0 {
            parts.append("\(attachment.eventCount) event\(attachment.eventCount == 1 ? "" : "s")")
        }
        if attachment.pendingCount > 0 {
            parts.append("\(attachment.pendingCount) pending")
        }
        if !attachment.summary.isEmpty {
            parts.append(attachment.summary)
        }
        return parts.joined(separator: " · ")
    }
}

struct GoalPlanPanel: View {
    var goal: GoalInfo?
    var plans: [GoalPlanInfo]
    var onActivate: (GoalPlanNodeInfo) -> Void

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: "checklist")
                .font(.system(size: 13))
                .foregroundStyle(Theme.accent)
                .frame(width: 18)
                .padding(.top, 2)
            VStack(alignment: .leading, spacing: 8) {
                VStack(alignment: .leading, spacing: 3) {
                    Text(goalTitle)
                        .font(.system(size: 12.5, weight: .semibold))
                        .foregroundStyle(Theme.textPrimary)
                        .lineLimit(1)
                    Text(summaryText)
                        .font(.system(size: 11.5))
                        .foregroundStyle(Theme.textSecondary)
                        .lineLimit(1)
                }
                ForEach(plans) { plan in
                    planRows(plan)
                }
            }
            Spacer(minLength: 8)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .fill(Theme.fieldFill)
                .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(Theme.cardStroke, lineWidth: 1))
        )
    }

    private var goalTitle: String {
        guard let objective = goal?.objective, !objective.isEmpty else { return "Goal plan" }
        return "Goal: \(objective)"
    }

    private var summaryText: String {
        let total = plans.reduce(0) { $0 + max($1.nodeCount, $1.nodes.count) }
        let done = plans.reduce(0) { $0 + $1.completedNodeCount }
        return total > 0 ? "\(done)/\(total) nodes complete" : "\(plans.count) goal plan\(plans.count == 1 ? "" : "s")"
    }

    private func planRows(_ plan: GoalPlanInfo) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(spacing: 6) {
                Text(plan.autoExecute.isEmpty ? "Plan" : plan.autoExecute)
                    .font(.system(size: 11.5, weight: .medium))
                    .foregroundStyle(Theme.textPrimary)
                    .lineLimit(1)
                Text(plan.progressText)
                    .font(.system(size: 11))
                    .foregroundStyle(Theme.textTertiary)
                    .lineLimit(1)
            }
            ForEach(Array(plan.nodes.prefix(4))) { node in
                nodeRow(node)
            }
        }
    }

    private func nodeRow(_ node: GoalPlanNodeInfo) -> some View {
        HStack(alignment: .center, spacing: 8) {
            Circle()
                .fill(node.ready ? Theme.accent : Theme.textTertiary.opacity(0.35))
                .frame(width: 6, height: 6)
            VStack(alignment: .leading, spacing: 1) {
                Text(node.key.isEmpty ? node.status : node.key)
                    .font(.system(size: 11.5, weight: .medium))
                    .foregroundStyle(Theme.textPrimary)
                    .lineLimit(1)
                Text(node.objective)
                    .font(.system(size: 11))
                    .foregroundStyle(Theme.textSecondary)
                    .lineLimit(1)
            }
            Spacer(minLength: 8)
            if node.canActivate {
                Button("Activate") { onActivate(node) }
                    .font(.system(size: 11.5, weight: .medium))
                    .foregroundStyle(.white)
                    .buttonStyle(.plain)
                    .padding(.horizontal, 10)
                    .frame(height: 24)
                    .background(Capsule().fill(Theme.accent))
            } else {
                Text(node.status)
                    .font(.system(size: 11))
                    .foregroundStyle(Theme.textTertiary)
            }
        }
    }
}

struct PendingServerRequestPanel: View {
    var prompt: PendingServerRequest
    var onAction: (PendingServerRequestAction) -> Void

    private var iconName: String {
        switch prompt.kind {
        case .commandApproval: return "terminal"
        case .fileChangeApproval: return "doc.badge.gearshape"
        case .permissionsApproval: return "lock.shield"
        }
    }

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: iconName)
                .font(.system(size: 13))
                .foregroundStyle(Theme.warning)
                .frame(width: 18)
                .padding(.top, 2)
            VStack(alignment: .leading, spacing: 4) {
                Text(prompt.title)
                    .font(.system(size: 12.5, weight: .semibold))
                    .foregroundStyle(Theme.textPrimary)
                Text(prompt.detail)
                    .font(.system(size: 11.5))
                    .foregroundStyle(Theme.textSecondary)
                    .lineLimit(3)
            }
            Spacer()
            HStack(spacing: 6) {
                ForEach(prompt.actions) { action in
                    Button(action.title) { onAction(action) }
                        .font(.system(size: 11.5, weight: .medium))
                        .foregroundStyle(action.isPrimary ? .white : Theme.textSecondary)
                        .buttonStyle(.plain)
                        .padding(.horizontal, 10)
                        .frame(height: 26)
                        .background(actionBackground(action))
                }
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .fill(Theme.fieldFill)
                .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(Theme.cardStroke, lineWidth: 1))
        )
    }

    private func actionBackground(_ action: PendingServerRequestAction) -> some View {
        Capsule()
            .fill(action.isPrimary ? Theme.accent : Theme.fieldFill)
            .overlay(Capsule().strokeBorder(action.isPrimary ? Color.clear : Theme.cardStroke, lineWidth: 1))
    }
}

struct PendingUserInputPanel: View {
    var prompt: PendingUserInputRequest
    var onSubmit: ([String: [String]]) -> Void
    var onCancel: () -> Void

    @State private var selectedOptions: [String: String] = [:]
    @State private var notes: [String: String] = [:]

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: "text.bubble")
                .font(.system(size: 13))
                .foregroundStyle(Theme.warning)
                .frame(width: 18)
                .padding(.top, 2)
            VStack(alignment: .leading, spacing: 8) {
                Text(prompt.title)
                    .font(.system(size: 12.5, weight: .semibold))
                    .foregroundStyle(Theme.textPrimary)
                ForEach(prompt.questions) { question in
                    questionView(question)
                }
            }
            Spacer(minLength: 8)
            HStack(spacing: 6) {
                Button("Cancel", action: onCancel)
                    .font(.system(size: 11.5, weight: .medium))
                    .foregroundStyle(Theme.textSecondary)
                    .buttonStyle(.plain)
                    .padding(.horizontal, 10)
                    .frame(height: 26)
                    .background(Capsule().fill(Theme.fieldFill).overlay(Capsule().strokeBorder(Theme.cardStroke, lineWidth: 1)))
                Button("Submit") { onSubmit(answerPayload()) }
                    .font(.system(size: 11.5, weight: .medium))
                    .foregroundStyle(.white)
                    .buttonStyle(.plain)
                    .padding(.horizontal, 10)
                    .frame(height: 26)
                    .background(Capsule().fill(Theme.accent))
                    .disabled(!canSubmit)
                    .opacity(canSubmit ? 1 : 0.45)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .fill(Theme.fieldFill)
                .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(Theme.cardStroke, lineWidth: 1))
        )
    }

    private func questionView(_ question: PendingUserInputQuestion) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            if !question.header.isEmpty {
                Text(question.header)
                    .font(.system(size: 11.5, weight: .medium))
                    .foregroundStyle(Theme.textPrimary)
                    .lineLimit(1)
            }
            Text(question.question)
                .font(.system(size: 11.5))
                .foregroundStyle(Theme.textSecondary)
                .fixedSize(horizontal: false, vertical: true)
            let options = displayOptions(for: question)
            if !options.isEmpty {
                HStack(spacing: 6) {
                    ForEach(Array(options.enumerated()), id: \.offset) { _, option in
                        optionButton(option, question: question)
                    }
                }
            }
            answerField(for: question, hasOptions: !options.isEmpty)
        }
    }

    private func optionButton(
        _ option: PendingUserInputOption,
        question: PendingUserInputQuestion
    ) -> some View {
        let selected = selectedOptions[question.id] == option.label
        return Button(option.label) {
            selectedOptions[question.id] = option.label
        }
        .font(.system(size: 11, weight: .medium))
        .foregroundStyle(selected ? .white : Theme.textSecondary)
        .buttonStyle(.plain)
        .padding(.horizontal, 8)
        .frame(height: 24)
        .background(
            Capsule()
                .fill(selected ? Theme.accent : Theme.fieldFill)
                .overlay(Capsule().strokeBorder(selected ? Color.clear : Theme.cardStroke, lineWidth: 1))
        )
    }

    @ViewBuilder
    private func answerField(for question: PendingUserInputQuestion, hasOptions: Bool) -> some View {
        let placeholder = hasOptions ? "Add notes" : "Type your answer"
        if question.isSecret {
            SecureField(placeholder, text: noteBinding(for: question))
                .textFieldStyle(.plain)
                .font(.system(size: 11.5))
                .padding(.horizontal, 8)
                .frame(height: 26)
                .background(fieldBackground)
        } else {
            TextField(placeholder, text: noteBinding(for: question), axis: .vertical)
                .textFieldStyle(.plain)
                .font(.system(size: 11.5))
                .lineLimit(1...2)
                .padding(.horizontal, 8)
                .frame(minHeight: 26)
                .background(fieldBackground)
        }
    }

    private var fieldBackground: some View {
        RoundedRectangle(cornerRadius: 7, style: .continuous)
            .fill(Theme.canvas)
            .overlay(RoundedRectangle(cornerRadius: 7, style: .continuous).strokeBorder(Theme.cardStroke, lineWidth: 1))
    }

    private func displayOptions(for question: PendingUserInputQuestion) -> [PendingUserInputOption] {
        var options = question.options
        if question.isOther {
            options.append(PendingUserInputOption(label: "Other", description: ""))
        }
        return options
    }

    private var canSubmit: Bool {
        prompt.questions.allSatisfy(hasAnswer)
    }

    private func hasAnswer(for question: PendingUserInputQuestion) -> Bool {
        if let selected = selectedOptions[question.id], !selected.isEmpty {
            return true
        }
        return !(notes[question.id] ?? "").trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    private func noteBinding(for question: PendingUserInputQuestion) -> Binding<String> {
        Binding(
            get: { notes[question.id] ?? "" },
            set: { notes[question.id] = $0 }
        )
    }

    private func answerPayload() -> [String: [String]] {
        var answers: [String: [String]] = [:]
        for question in prompt.questions {
            var values: [String] = []
            if let selected = selectedOptions[question.id], !selected.isEmpty {
                values.append(selected)
            }
            let note = (notes[question.id] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            if !note.isEmpty {
                values.append("user_note: \(note)")
            }
            answers[question.id] = values
        }
        return answers
    }
}

struct PendingMcpElicitationPanel: View {
    var prompt: PendingMcpElicitationRequest
    var onOpenURL: () -> Void
    var onAccept: (JSONValue) -> Void
    var onDecline: () -> Void
    var onCancel: () -> Void

    @State private var textValues: [String: String] = [:]
    @State private var selectedValues: [String: Set<String>] = [:]

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: iconName)
                .font(.system(size: 13))
                .foregroundStyle(Theme.warning)
                .frame(width: 18)
                .padding(.top, 2)
            VStack(alignment: .leading, spacing: 8) {
                Text(prompt.title)
                    .font(.system(size: 12.5, weight: .semibold))
                    .foregroundStyle(Theme.textPrimary)
                Text(prompt.message)
                    .font(.system(size: 11.5))
                    .foregroundStyle(Theme.textSecondary)
                    .fixedSize(horizontal: false, vertical: true)
                ForEach(prompt.fields) { field in
                    fieldView(field)
                }
            }
            Spacer(minLength: 8)
            actions
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .fill(Theme.fieldFill)
                .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(Theme.cardStroke, lineWidth: 1))
        )
    }

    private var iconName: String {
        switch prompt.mode {
        case .form: return "server.rack"
        case .url: return "link"
        }
    }

    @ViewBuilder
    private var actions: some View {
        switch prompt.mode {
        case .form:
            HStack(spacing: 6) {
                Button("Decline", action: onDecline)
                    .font(.system(size: 11.5, weight: .medium))
                    .foregroundStyle(Theme.textSecondary)
                    .buttonStyle(.plain)
                    .padding(.horizontal, 10)
                    .frame(height: 26)
                    .background(Capsule().fill(Theme.fieldFill).overlay(Capsule().strokeBorder(Theme.cardStroke, lineWidth: 1)))
                Button("Cancel", action: onCancel)
                    .font(.system(size: 11.5, weight: .medium))
                    .foregroundStyle(Theme.textSecondary)
                    .buttonStyle(.plain)
                    .padding(.horizontal, 10)
                    .frame(height: 26)
                    .background(Capsule().fill(Theme.fieldFill).overlay(Capsule().strokeBorder(Theme.cardStroke, lineWidth: 1)))
                Button("Submit") { onAccept(contentPayload()) }
                    .font(.system(size: 11.5, weight: .medium))
                    .foregroundStyle(.white)
                    .buttonStyle(.plain)
                    .padding(.horizontal, 10)
                    .frame(height: 26)
                    .background(Capsule().fill(canSubmit ? Theme.accent : Theme.textTertiary))
                    .disabled(!canSubmit)
            }
        case .url:
            HStack(spacing: 6) {
                Button("Decline", action: onDecline)
                    .font(.system(size: 11.5, weight: .medium))
                    .foregroundStyle(Theme.textSecondary)
                    .buttonStyle(.plain)
                    .padding(.horizontal, 10)
                    .frame(height: 26)
                    .background(Capsule().fill(Theme.fieldFill).overlay(Capsule().strokeBorder(Theme.cardStroke, lineWidth: 1)))
                Button("Cancel", action: onCancel)
                    .font(.system(size: 11.5, weight: .medium))
                    .foregroundStyle(Theme.textSecondary)
                    .buttonStyle(.plain)
                    .padding(.horizontal, 10)
                    .frame(height: 26)
                    .background(Capsule().fill(Theme.fieldFill).overlay(Capsule().strokeBorder(Theme.cardStroke, lineWidth: 1)))
                Button("Open", action: onOpenURL)
                    .font(.system(size: 11.5, weight: .medium))
                    .foregroundStyle(Theme.textSecondary)
                    .buttonStyle(.plain)
                    .padding(.horizontal, 10)
                    .frame(height: 26)
                    .background(Capsule().fill(Theme.fieldFill).overlay(Capsule().strokeBorder(Theme.cardStroke, lineWidth: 1)))
                Button("Done") { onAccept(.null) }
                    .font(.system(size: 11.5, weight: .medium))
                    .foregroundStyle(.white)
                    .buttonStyle(.plain)
                    .padding(.horizontal, 10)
                    .frame(height: 26)
                    .background(Capsule().fill(Theme.accent))
            }
        }
    }

    @ViewBuilder
    private func fieldView(_ field: PendingMcpElicitationField) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(field.label)
                .font(.system(size: 11.5, weight: .medium))
                .foregroundStyle(Theme.textPrimary)
                .lineLimit(1)
            Text(field.prompt)
                .font(.system(size: 11.5))
                .foregroundStyle(Theme.textSecondary)
                .fixedSize(horizontal: false, vertical: true)
            switch field.kind {
            case .text, .number, .integer:
                TextField(textPlaceholder(for: field), text: textBinding(for: field), axis: .vertical)
                    .textFieldStyle(.plain)
                    .font(.system(size: 11.5))
                    .lineLimit(1...2)
                    .padding(.horizontal, 8)
                    .frame(minHeight: 26)
                    .background(fieldBackground)
            case .secret:
                SecureField(textPlaceholder(for: field), text: textBinding(for: field))
                    .textFieldStyle(.plain)
                    .font(.system(size: 11.5))
                    .padding(.horizontal, 8)
                    .frame(height: 26)
                    .background(fieldBackground)
            case .singleSelect, .multiSelect:
                HStack(spacing: 6) {
                    ForEach(field.options) { option in
                        optionButton(option, field: field)
                    }
                }
            }
        }
    }

    private func optionButton(
        _ option: PendingMcpElicitationOption,
        field: PendingMcpElicitationField
    ) -> some View {
        let selected = isSelected(option, field: field)
        return Button(option.label) {
            toggle(option, field: field)
        }
        .font(.system(size: 11, weight: .medium))
        .foregroundStyle(selected ? .white : Theme.textSecondary)
        .buttonStyle(.plain)
        .padding(.horizontal, 8)
        .frame(height: 24)
        .background(
            Capsule()
                .fill(selected ? Theme.accent : Theme.fieldFill)
                .overlay(Capsule().strokeBorder(selected ? Color.clear : Theme.cardStroke, lineWidth: 1))
        )
    }

    private var fieldBackground: some View {
        RoundedRectangle(cornerRadius: 7, style: .continuous)
            .fill(Theme.canvas)
            .overlay(RoundedRectangle(cornerRadius: 7, style: .continuous).strokeBorder(Theme.cardStroke, lineWidth: 1))
    }

    private var canSubmit: Bool {
        prompt.fields.allSatisfy { field in
            !field.required || fieldValue(field) != nil || field.defaultValue != nil
        }
    }

    private func contentPayload() -> JSONValue {
        var values: [String: JSONValue] = [:]
        for field in prompt.fields {
            if let value = fieldValue(field) {
                values[field.id] = value
            }
        }
        return AppModel.mcpElicitationContent(for: prompt, values: values)
    }

    private func fieldValue(_ field: PendingMcpElicitationField) -> JSONValue? {
        switch field.kind {
        case .text, .secret:
            let value = (textValues[field.id] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            return value.isEmpty ? nil : .string(value)
        case .number:
            let value = (textValues[field.id] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            guard !value.isEmpty, let number = Double(value) else { return nil }
            return .number(number)
        case .integer:
            let value = (textValues[field.id] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            guard !value.isEmpty, let integer = Int(value) else { return nil }
            return .number(Double(integer))
        case .singleSelect:
            return selectedOption(for: field)?.value
        case .multiSelect:
            let selected = selectedValues[field.id]
                ?? Set(field.options.filter(\.isDefault).map(\.label))
            let values = field.options.filter { selected.contains($0.label) }.map(\.value)
            return values.isEmpty ? nil : .array(values)
        }
    }

    private func selectedOption(for field: PendingMcpElicitationField) -> PendingMcpElicitationOption? {
        if let selected = selectedValues[field.id]?.first {
            return field.options.first { $0.label == selected }
        }
        return field.options.first { $0.isDefault }
    }

    private func isSelected(_ option: PendingMcpElicitationOption, field: PendingMcpElicitationField) -> Bool {
        if let selected = selectedValues[field.id], !selected.isEmpty {
            return selected.contains(option.label)
        }
        return option.isDefault
    }

    private func toggle(_ option: PendingMcpElicitationOption, field: PendingMcpElicitationField) {
        switch field.kind {
        case .multiSelect:
            var selected = selectedValues[field.id] ?? Set(field.options.filter(\.isDefault).map(\.label))
            if selected.contains(option.label) {
                selected.remove(option.label)
            } else {
                selected.insert(option.label)
            }
            selectedValues[field.id] = selected
        default:
            selectedValues[field.id] = [option.label]
        }
    }

    private func textBinding(for field: PendingMcpElicitationField) -> Binding<String> {
        Binding(
            get: { textValues[field.id] ?? "" },
            set: { textValues[field.id] = $0 }
        )
    }

    private func textPlaceholder(for field: PendingMcpElicitationField) -> String {
        switch field.kind {
        case .number:
            return "Enter a number"
        case .integer:
            return "Enter a whole number"
        default:
            return "Enter a value"
        }
    }
}

struct ToolRow: View {
    var icon: String
    var text: String
    var body: some View {
        // Flat inline tool line (icon + low-contrast text), no chip fill — the
        // reference renders tool calls as plain gray lines, not filled chips.
        let lineLimit = icon == "terminal" ? 6 : 1
        HStack(spacing: 6) {
            Image(systemName: icon).font(.system(size: 11)).foregroundStyle(Theme.textTertiary)
            Text(text).font(.system(size: 12)).foregroundStyle(Theme.textSecondary).lineLimit(lineLimit)
        }
        .padding(.vertical, 3)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.bottom, 8)
    }
}
