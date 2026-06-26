import SwiftUI

/// Loops — recurring schedules + monitors running across all sessions (live data).
struct LoopsView: View {
    var loops: [LoopInfo] = []
    var error: String? = nil
    var onToggle: (LoopInfo) -> Void = { _ in }
    var onCreate: (LoopCreationDraft) -> Void = { _ in }
    var onRunNow: (LoopInfo) -> Void = { _ in }
    var onDelete: (LoopInfo) -> Void = { _ in }
    @State private var pendingDeleteLoop: LoopInfo?
    @State private var showingCreateLoop = false
    @State private var draft = LoopCreationDraft()

    var body: some View {
        VStack(spacing: 0) {
            topBar
            Rectangle().fill(Theme.separator).frame(height: 1)
            ScrollColumn(spacing: 0) {
                VStack(alignment: .leading, spacing: 8) {
                    if let error {
                        Text("Loops unavailable: \(error)")
                            .font(.system(size: 12))
                            .foregroundStyle(Theme.textTertiary)
                            .padding(.top, 8)
                    } else if loops.isEmpty {
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
        .confirmationDialog("Delete loop?", isPresented: deleteConfirmationBinding) {
            Button("Delete", role: .destructive) {
                if let pendingDeleteLoop { onDelete(pendingDeleteLoop) }
                pendingDeleteLoop = nil
            }
            Button("Cancel", role: .cancel) { pendingDeleteLoop = nil }
        } message: {
            Text("This removes the loop from its thread.")
        }
        .sheet(isPresented: $showingCreateLoop) {
            createLoopSheet
        }
    }

    private var deleteConfirmationBinding: Binding<Bool> {
        Binding(
            get: { pendingDeleteLoop != nil },
            set: { isPresented in
                if !isPresented { pendingDeleteLoop = nil }
            }
        )
    }

    private var topBar: some View {
        HStack {
            Text("Loops").font(.system(size: 13)).foregroundStyle(Theme.textSecondary)
            Spacer()
            Button {
                draft = LoopCreationDraft()
                showingCreateLoop = true
            } label: {
                HStack(spacing: 5) {
                    Image(systemName: "plus").font(.system(size: 10, weight: .semibold))
                    Text("New loop").font(.system(size: 11.5, weight: .medium))
                }
                .foregroundStyle(.white)
                .padding(.horizontal, 12).frame(height: 26)
                .background(Capsule().fill(Color(hex: 0x202020)))
            }
            .buttonStyle(.plain)
        }
        .padding(.horizontal, 22).frame(height: 40)
    }

    private var createLoopSheet: some View {
        VStack(alignment: .leading, spacing: 16) {
            VStack(alignment: .leading, spacing: 4) {
                Text("New loop")
                    .font(.system(size: 17, weight: .semibold))
                    .foregroundStyle(Theme.textPrimary)
                Text("Create a schedule or monitor through the CodeWith app-server.")
                    .font(.system(size: 11.5))
                    .foregroundStyle(Theme.textSecondary)
            }

            HStack(spacing: 8) {
                ForEach(LoopCreationKind.allCases, id: \.self) { kind in
                    choiceButton(title: kindTitle(kind), selected: draft.kind == kind) {
                        draft.kind = kind
                    }
                }
            }

            field("Prompt") {
                TextField(LoopCreationDraft.defaultPrompt, text: $draft.prompt, axis: .vertical)
                    .textFieldStyle(.plain)
                    .font(.system(size: 12.5))
                    .lineLimit(2...4)
            }

            switch draft.kind {
            case .schedule:
                scheduleFields
            case .monitor:
                monitorFields
            }

            if let message = draft.validationMessage {
                Text(message)
                    .font(.system(size: 11.5))
                    .foregroundStyle(Theme.textTertiary)
            }

            HStack {
                Spacer()
                Button("Cancel") { showingCreateLoop = false }
                    .buttonStyle(.plain)
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(Theme.textSecondary)
                    .padding(.horizontal, 12)
                    .frame(height: 28)
                Button {
                    onCreate(draft)
                    showingCreateLoop = false
                } label: {
                    Text("Create")
                        .font(.system(size: 12, weight: .medium))
                        .foregroundStyle(.white)
                        .padding(.horizontal, 14)
                        .frame(height: 28)
                        .background(Capsule().fill(Color(hex: 0x202020)))
                }
                .buttonStyle(.plain)
                .disabled(!draft.canCreate)
                .opacity(draft.canCreate ? 1 : 0.45)
            }
        }
        .padding(20)
        .frame(width: 420)
        .background(Theme.canvas)
    }

    private var scheduleFields: some View {
        VStack(alignment: .leading, spacing: 12) {
            fieldLabel("Schedule")
            HStack(spacing: 8) {
                ForEach(LoopCreationScheduleMode.allCases, id: \.self) { mode in
                    choiceButton(title: scheduleModeTitle(mode), selected: draft.scheduleMode == mode) {
                        draft.scheduleMode = mode
                    }
                }
            }

            switch draft.scheduleMode {
            case .dynamic:
                EmptyView()
            case .interval:
                HStack(spacing: 8) {
                    field("Every") {
                        TextField("5", text: $draft.intervalAmount)
                            .textFieldStyle(.plain)
                            .font(.system(size: 12.5))
                            .frame(width: 64)
                    }
                    Menu {
                        Button("Minutes") { draft.intervalUnit = .minutes }
                        Button("Hours") { draft.intervalUnit = .hours }
                        Button("Days") { draft.intervalUnit = .days }
                    } label: {
                        DropdownPill(text: intervalUnitTitle(draft.intervalUnit), minWidth: 110)
                    }
                    .menuStyle(.borderlessButton)
                    .menuIndicator(.hidden)
                }
            case .cron:
                field("Cron") {
                    TextField("*/15 * * * *", text: $draft.cronExpression)
                        .textFieldStyle(.plain)
                        .font(.system(size: 12.5, design: .monospaced))
                }
            }
        }
    }

    private var monitorFields: some View {
        VStack(alignment: .leading, spacing: 12) {
            field("Name") {
                TextField("Watch logs", text: $draft.monitorName)
                    .textFieldStyle(.plain)
                    .font(.system(size: 12.5))
            }
            field("Command") {
                TextField("tail -f app.log", text: $draft.command, axis: .vertical)
                    .textFieldStyle(.plain)
                    .font(.system(size: 12.5, design: .monospaced))
                    .lineLimit(1...3)
            }
            field("Working directory") {
                TextField("Optional", text: $draft.cwd)
                    .textFieldStyle(.plain)
                    .font(.system(size: 12.5, design: .monospaced))
            }
            fieldLabel("Routing")
            HStack(spacing: 8) {
                ForEach(LoopMonitorRouting.allCases, id: \.self) { routing in
                    choiceButton(title: routingTitle(routing), selected: draft.routing == routing) {
                        draft.routing = routing
                        if !routing.writesToFile { draft.outputFile = "" }
                    }
                }
            }
            if draft.routing.writesToFile {
                field("Output file") {
                    TextField("monitor.log", text: $draft.outputFile)
                        .textFieldStyle(.plain)
                        .font(.system(size: 12.5, design: .monospaced))
                }
            }
        }
    }

    private func field<Content: View>(_ title: String, @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            fieldLabel(title)
            content()
                .foregroundStyle(Theme.textPrimary)
                .padding(.horizontal, 10)
                .frame(minHeight: 32)
                .background(
                    RoundedRectangle(cornerRadius: 7)
                        .fill(Theme.fieldFill)
                        .overlay(RoundedRectangle(cornerRadius: 7).strokeBorder(Theme.cardStroke, lineWidth: 1))
                )
        }
    }

    private func fieldLabel(_ title: String) -> some View {
        Text(title)
            .font(.system(size: 12))
            .foregroundStyle(Theme.textSecondary)
    }

    private func choiceButton(title: String, selected: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title)
                .font(.system(size: 12, weight: .medium))
                .foregroundStyle(selected ? Theme.textPrimary : Theme.textSecondary)
                .padding(.horizontal, 10)
                .frame(height: 28)
                .background(
                    Capsule()
                        .fill(selected ? Theme.fieldFill : .clear)
                        .overlay(Capsule().strokeBorder(selected ? Theme.cardStroke : Theme.separator, lineWidth: 1))
                )
        }
        .buttonStyle(.plain)
    }

    private func kindTitle(_ kind: LoopCreationKind) -> String {
        switch kind {
        case .schedule: return "Schedule"
        case .monitor: return "Monitor"
        }
    }

    private func scheduleModeTitle(_ mode: LoopCreationScheduleMode) -> String {
        switch mode {
        case .dynamic: return "Dynamic"
        case .interval: return "Interval"
        case .cron: return "Cron"
        }
    }

    private func intervalUnitTitle(_ unit: LoopScheduleIntervalUnit) -> String {
        switch unit {
        case .minutes: return "Minutes"
        case .hours: return "Hours"
        case .days: return "Days"
        }
    }

    private func routingTitle(_ routing: LoopMonitorRouting) -> String {
        switch routing {
        case .stream: return "Stream"
        case .file: return "File"
        case .both: return "Both"
        }
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
            GlassToggle(on: loop.active) { if loop.canToggle { onToggle(loop) } }
                .disabled(!loop.canToggle)
                .opacity(loop.canToggle ? 1 : 0.45)
        }
        .padding(.horizontal, 12).padding(.vertical, 11)
        .background(
            RoundedRectangle(cornerRadius: Theme.cardRadius, style: .continuous)
                .fill(Theme.fieldFill)
                .overlay(RoundedRectangle(cornerRadius: Theme.cardRadius, style: .continuous).strokeBorder(Theme.cardStroke, lineWidth: 1))
        )
        .contextMenu {
            if loop.kind == .schedule {
                Button("Run now") { onRunNow(loop) }
                    .disabled(!loop.canRunNow)
            }
            Button(loop.toggleLabel) { onToggle(loop) }
                .disabled(!loop.canToggle)
            Divider()
            Button("Delete") { pendingDeleteLoop = loop }
        }
    }
}
