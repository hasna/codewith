import SwiftUI

/// A live session view, driven by `model.activeMessages`.
struct ChatView: View {
    @Bindable var model: AppModel
    var threadId: String
    var onSubmit: () -> Void = {}
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
                Button(action: onToggleConfig) {
                    Image(systemName: "sidebar.right").font(.system(size: 13)).foregroundStyle(Theme.textTertiary)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
            .padding(.horizontal, 16).frame(height: 40)
            Rectangle().fill(Theme.separator).frame(height: 1)

            // Conversation
            ScrollColumn(alignment: .leading, spacing: 0) {
                if model.activeMessages.isEmpty {
                    Text(model.turnInProgress ? "Working…" : "")
                        .font(.system(size: 12)).foregroundStyle(Theme.textTertiary)
                        .padding(.top, 8)
                } else {
                    ForEach(model.activeMessages) { messageView($0) }
                    if model.turnInProgress {
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
                    onApprove: { model.respondToServerRequest(pending, approve: true) },
                    onDecline: { model.respondToServerRequest(pending, approve: false) }
                )
                .padding(.horizontal, 24)
                .padding(.top, 6)
            }
            if let prompt = model.pendingUserInputForActiveThread {
                PendingUserInputPanel(
                    prompt: prompt,
                    onSubmit: { answers in model.respondToUserInputRequest(prompt, answers: answers) },
                    onCancel: { model.cancelUserInputRequest(prompt) }
                )
                .padding(.horizontal, 24)
                .padding(.top, 6)
            }
            if let prompt = model.pendingMcpElicitationForActiveThread {
                PendingMcpElicitationPanel(
                    prompt: prompt,
                    onOpenURL: { model.openMcpElicitationURL(prompt) },
                    onSubmit: { values in
                        model.respondToMcpElicitationRequest(
                            prompt,
                            action: "accept",
                            content: AppModel.mcpElicitationContent(for: prompt, values: values))
                    },
                    onDecline: { model.respondToMcpElicitationRequest(prompt, action: "decline") }
                )
                .padding(.horizontal, 24)
                .padding(.top, 6)
            }

            // Composer
            Composer(placeholder: "Ask for follow-up changes",
                     stopMode: model.turnInProgress,
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

struct PendingUserInputPanel: View {
    var prompt: PendingUserInputRequest
    var onSubmit: ([String: [String]]) -> Void
    var onCancel: () -> Void
    @State private var answers: [String: String] = [:]

    var body: some View {
        PendingPromptShell(icon: "questionmark.circle", title: prompt.title) {
            VStack(alignment: .leading, spacing: 8) {
                ForEach(prompt.questions) { question in
                    VStack(alignment: .leading, spacing: 5) {
                        Text(question.question)
                            .font(.system(size: 11.5))
                            .foregroundStyle(Theme.textSecondary)
                        if question.options.isEmpty {
                            if question.isSecret {
                                SecureField("Answer", text: answerBinding(question.id))
                                    .textFieldStyle(.plain)
                                    .font(.system(size: 11.5))
                            } else {
                                TextField("Answer", text: answerBinding(question.id))
                                    .textFieldStyle(.plain)
                                    .font(.system(size: 11.5))
                            }
                        } else {
                            HStack(spacing: 6) {
                                ForEach(question.options) { option in
                                    Button(option.label) { answers[question.id] = option.label }
                                        .font(.system(size: 11.5, weight: .medium))
                                        .foregroundStyle(answers[question.id] == option.label ? .white : Theme.textSecondary)
                                        .buttonStyle(.plain)
                                        .padding(.horizontal, 10)
                                        .frame(height: 24)
                                        .background(Capsule().fill(answers[question.id] == option.label ? Theme.accent : Theme.fieldFill)
                                            .overlay(Capsule().strokeBorder(Theme.cardStroke, lineWidth: 1)))
                                }
                            }
                        }
                    }
                }
            }
        } actions: {
            Button("Cancel", action: onCancel)
                .font(.system(size: 11.5, weight: .medium))
                .foregroundStyle(Theme.textSecondary)
                .buttonStyle(.plain)
                .padding(.horizontal, 10)
                .frame(height: 26)
                .background(Capsule().fill(Theme.fieldFill).overlay(Capsule().strokeBorder(Theme.cardStroke, lineWidth: 1)))
            Button("Submit") {
                onSubmit(answers.mapValues { value in value.isEmpty ? [] : [value] })
            }
            .font(.system(size: 11.5, weight: .medium))
            .foregroundStyle(.white)
            .buttonStyle(.plain)
            .padding(.horizontal, 10)
            .frame(height: 26)
            .background(Capsule().fill(Theme.accent))
        }
    }

    private func answerBinding(_ id: String) -> Binding<String> {
        Binding(
            get: { answers[id] ?? "" },
            set: { answers[id] = $0 }
        )
    }
}

struct PendingMcpElicitationPanel: View {
    var prompt: PendingMcpElicitationRequest
    var onOpenURL: () -> Void
    var onSubmit: ([String: JSONValue]) -> Void
    var onDecline: () -> Void
    @State private var values: [String: String] = [:]
    @State private var selectedValues: [String: JSONValue] = [:]

    var body: some View {
        PendingPromptShell(icon: "puzzlepiece.extension", title: prompt.title) {
            VStack(alignment: .leading, spacing: 8) {
                Text(prompt.message)
                    .font(.system(size: 11.5))
                    .foregroundStyle(Theme.textSecondary)
                switch prompt.mode {
                case .url(let url):
                    Text(url)
                        .font(.system(size: 11))
                        .foregroundStyle(Theme.textTertiary)
                        .lineLimit(1)
                case .form:
                    ForEach(prompt.fields) { field in
                        mcpField(field)
                    }
                }
            }
        } actions: {
            Button("Decline", action: onDecline)
                .font(.system(size: 11.5, weight: .medium))
                .foregroundStyle(Theme.textSecondary)
                .buttonStyle(.plain)
                .padding(.horizontal, 10)
                .frame(height: 26)
                .background(Capsule().fill(Theme.fieldFill).overlay(Capsule().strokeBorder(Theme.cardStroke, lineWidth: 1)))
            if case .url = prompt.mode {
                Button("Open", action: onOpenURL)
                    .font(.system(size: 11.5, weight: .medium))
                    .foregroundStyle(.white)
                    .buttonStyle(.plain)
                    .padding(.horizontal, 10)
                    .frame(height: 26)
                    .background(Capsule().fill(Theme.accent))
            } else {
                Button("Submit") {
                    var content = selectedValues
                    for (key, value) in values where !value.isEmpty {
                        content[key] = .string(value)
                    }
                    onSubmit(content)
                }
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
    private func mcpField(_ field: PendingMcpElicitationField) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(field.label)
                .font(.system(size: 11.5))
                .foregroundStyle(Theme.textSecondary)
            switch field.kind {
            case .singleSelect, .multiSelect:
                HStack(spacing: 6) {
                    ForEach(field.options) { option in
                        Button(option.label) { selectedValues[field.id] = option.value }
                            .font(.system(size: 11.5, weight: .medium))
                            .foregroundStyle(selectedValues[field.id] == option.value ? .white : Theme.textSecondary)
                            .buttonStyle(.plain)
                            .padding(.horizontal, 10)
                            .frame(height: 24)
                            .background(Capsule().fill(selectedValues[field.id] == option.value ? Theme.accent : Theme.fieldFill)
                                .overlay(Capsule().strokeBorder(Theme.cardStroke, lineWidth: 1)))
                    }
                }
            case .secret:
                SecureField(field.prompt, text: valueBinding(field.id))
                    .textFieldStyle(.plain)
                    .font(.system(size: 11.5))
            default:
                TextField(field.prompt, text: valueBinding(field.id))
                    .textFieldStyle(.plain)
                    .font(.system(size: 11.5))
            }
        }
    }

    private func valueBinding(_ id: String) -> Binding<String> {
        Binding(
            get: { values[id] ?? "" },
            set: { values[id] = $0 }
        )
    }
}

struct PendingPromptShell<Content: View, Actions: View>: View {
    var icon: String
    var title: String
    @ViewBuilder var content: () -> Content
    @ViewBuilder var actions: () -> Actions

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: icon)
                .font(.system(size: 13))
                .foregroundStyle(Theme.warning)
                .frame(width: 18)
                .padding(.top, 2)
            VStack(alignment: .leading, spacing: 4) {
                Text(title)
                    .font(.system(size: 12.5, weight: .semibold))
                    .foregroundStyle(Theme.textPrimary)
                content()
            }
            Spacer()
            HStack(spacing: 6) {
                actions()
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
}

struct PendingServerRequestPanel: View {
    var prompt: PendingServerRequest
    var onApprove: () -> Void
    var onDecline: () -> Void

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
                Button("Decline", action: onDecline)
                    .font(.system(size: 11.5, weight: .medium))
                    .foregroundStyle(Theme.textSecondary)
                    .buttonStyle(.plain)
                    .padding(.horizontal, 10)
                    .frame(height: 26)
                    .background(Capsule().fill(Theme.fieldFill).overlay(Capsule().strokeBorder(Theme.cardStroke, lineWidth: 1)))
                Button("Approve", action: onApprove)
                    .font(.system(size: 11.5, weight: .medium))
                    .foregroundStyle(.white)
                    .buttonStyle(.plain)
                    .padding(.horizontal, 10)
                    .frame(height: 26)
                    .background(Capsule().fill(Theme.accent))
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
}

struct ToolRow: View {
    var icon: String
    var text: String
    var body: some View {
        // Flat inline tool line (icon + low-contrast text), no chip fill; the
        // reference renders tool calls as plain gray lines, not filled chips.
        HStack(spacing: 6) {
            Image(systemName: icon).font(.system(size: 11)).foregroundStyle(Theme.textTertiary)
            Text(text).font(.system(size: 12)).foregroundStyle(Theme.textSecondary).lineLimit(1)
        }
        .padding(.vertical, 3)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.bottom, 8)
    }
}
