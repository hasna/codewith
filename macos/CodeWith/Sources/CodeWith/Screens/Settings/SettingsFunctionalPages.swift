import SwiftUI

struct SettingsKeyboardShortcuts: View {
    private let shortcuts: [(String, String)] = [
        ("New chat", "Command N"),
        ("Search", "Command K"),
        ("Send message", "Return"),
        ("New line in composer", "Shift Return"),
        ("Toggle config panel", "Command ,"),
    ]

    var body: some View {
        SettingsPage(title: "Keyboard shortcuts") {
            VStack(spacing: 0) {
                ForEach(Array(shortcuts.enumerated()), id: \.offset) { index, shortcut in
                    SettingsRow(title: shortcut.0, showDivider: index < shortcuts.count - 1) {
                        Text(shortcut.1)
                            .font(.system(size: 12, weight: .medium))
                            .foregroundStyle(Theme.textPrimary)
                            .padding(.horizontal, 10)
                            .frame(height: 26)
                            .background(RoundedRectangle(cornerRadius: 6).fill(Theme.fieldFill))
                    }
                }
            }
        }
    }
}

struct SettingsUsageBilling: View {
    var account: AccountInfo? = nil
    var activeProfile: AuthProfileInfo? = nil
    var usage: AccountUsageInfo? = nil
    var error: String? = nil
    var onRefresh: () -> Void = {}

    var body: some View {
        SettingsPage(title: "Usage & billing", subtitle: accountScope) {
            VStack(alignment: .leading, spacing: 16) {
                HStack {
                    SettingsGroupLabel(text: "Account usage")
                    Spacer()
                    refreshButton
                }

                if let error {
                    statusRow(error, icon: "exclamationmark.triangle.fill", color: Theme.warning)
                }

                if let usage {
                    usageSummary(usage)
                    dailyUsage(usage.dailyBuckets)
                } else {
                    statusRow("No usage data was returned for the active account.", icon: "chart.bar", color: Theme.textTertiary)
                }
            }
        }
    }

    private var accountScope: String {
        let accountPart = account?.email.isEmpty == false ? account?.email : account?.name
        let profilePart = activeProfile?.name
        return [accountPart, profilePart.map { "profile: \($0)" }]
            .compactMap { $0 }
            .filter { !$0.isEmpty }
            .joined(separator: " · ")
    }

    private var refreshButton: some View {
        Button(action: onRefresh) {
            Label("Refresh", systemImage: "arrow.clockwise")
                .font(.system(size: 12, weight: .medium))
        }
        .buttonStyle(.borderless)
    }

    private func usageSummary(_ usage: AccountUsageInfo) -> some View {
        VStack(spacing: 0) {
            metricRow("Lifetime tokens", value: formattedCount(usage.lifetimeTokens))
            metricRow("Peak daily tokens", value: formattedCount(usage.peakDailyTokens))
            metricRow("Longest running turn", value: formattedDuration(usage.longestRunningTurnSec))
            metricRow("Current streak", value: formattedDays(usage.currentStreakDays))
            metricRow("Longest streak", value: formattedDays(usage.longestStreakDays), showDivider: false)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 4)
        .background(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .fill(Color.white)
                .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(Theme.cardStroke, lineWidth: 1))
        )
    }

    private func dailyUsage(_ buckets: [AccountUsageBucket]) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Recent daily usage")
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(Theme.textPrimary)

            if buckets.isEmpty {
                Text("No daily usage buckets were returned.")
                    .font(.system(size: 12))
                    .foregroundStyle(Theme.textTertiary)
            } else {
                let maxTokens = max(buckets.map(\.tokens).max() ?? 1, 1)
                VStack(spacing: 6) {
                    ForEach(buckets.suffix(10)) { bucket in
                        HStack(spacing: 8) {
                            Text(bucket.startDate)
                                .font(.system(size: 11))
                                .foregroundStyle(Theme.textSecondary)
                                .frame(width: 88, alignment: .leading)
                            GeometryReader { geometry in
                                RoundedRectangle(cornerRadius: 4)
                                    .fill(Theme.toggleBlue.opacity(0.18))
                                    .overlay(alignment: .leading) {
                                        RoundedRectangle(cornerRadius: 4)
                                            .fill(Theme.toggleBlue)
                                            .frame(width: geometry.size.width * CGFloat(bucket.tokens) / CGFloat(maxTokens))
                                    }
                            }
                            .frame(height: 8)
                            Text(formattedCount(bucket.tokens))
                                .font(.system(size: 11))
                                .foregroundStyle(Theme.textSecondary)
                                .frame(width: 72, alignment: .trailing)
                        }
                    }
                }
            }
        }
    }

    private func metricRow(_ title: String, value: String, showDivider: Bool = true) -> some View {
        SettingsRow(title: title, showDivider: showDivider) {
            Text(value)
                .font(.system(size: 12, weight: .medium))
                .foregroundStyle(Theme.textPrimary)
        }
    }
}

struct SettingsMcpServers: View {
    var servers: [McpServerStatusInfo] = []
    var error: String? = nil
    var onRefresh: () -> Void = {}

    var body: some View {
        SettingsPage(title: "MCP servers", subtitle: "Servers currently reported by the app-server.") {
            refreshableListHeader(title: "Configured servers", onRefresh: onRefresh)
            if let error {
                statusRow(error, icon: "exclamationmark.triangle.fill", color: Theme.warning)
            } else if servers.isEmpty {
                statusRow("No MCP servers are configured for the active session.", icon: "antenna.radiowaves.left.and.right", color: Theme.textTertiary)
            } else {
                VStack(spacing: 8) {
                    ForEach(servers) { server in
                        infoRow(
                            icon: "antenna.radiowaves.left.and.right",
                            title: server.name,
                            subtitle: "\(server.authStatus) · \(server.toolCount) tools · \(server.resourceCount) resources")
                    }
                }
            }
        }
    }
}

struct SettingsHooks: View {
    var entries: [HookEntryInfo] = []
    var error: String? = nil
    var onRefresh: () -> Void = {}

    var body: some View {
        SettingsPage(title: "Hooks", subtitle: "Hook configuration reported for the active workspace.") {
            refreshableListHeader(title: "Workspace hooks", onRefresh: onRefresh)
            if let error {
                statusRow(error, icon: "exclamationmark.triangle.fill", color: Theme.warning)
            } else if entries.isEmpty {
                statusRow("No hooks were returned for the active workspace.", icon: "bolt", color: Theme.textTertiary)
            } else {
                VStack(spacing: 10) {
                    ForEach(entries) { entry in
                        hookEntry(entry)
                    }
                }
            }
        }
    }

    private func hookEntry(_ entry: HookEntryInfo) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(entry.cwd.isEmpty ? "Current workspace" : entry.cwd)
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(Theme.textPrimary)
                .lineLimit(1)

            if entry.hooks.isEmpty {
                Text("No hooks in this location.")
                    .font(.system(size: 12))
                    .foregroundStyle(Theme.textTertiary)
            } else {
                ForEach(entry.hooks) { hook in
                    HStack(spacing: 8) {
                        Image(systemName: hook.enabled ? "checkmark.circle.fill" : "circle")
                            .font(.system(size: 12))
                            .foregroundStyle(hook.enabled ? Theme.success : Theme.textTertiary)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(hook.eventName.isEmpty ? hook.key : hook.eventName)
                                .font(.system(size: 12, weight: .medium))
                                .foregroundStyle(Theme.textPrimary)
                            Text(hook.command ?? hook.handlerType)
                                .font(.system(size: 11))
                                .foregroundStyle(Theme.textSecondary)
                                .lineLimit(1)
                        }
                        Spacer()
                        Text(hook.trustStatus)
                            .font(.system(size: 11))
                            .foregroundStyle(Theme.textTertiary)
                    }
                }
            }
        }
        .padding(12)
        .background(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .fill(Theme.fieldFill)
                .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(Theme.cardStroke, lineWidth: 1))
        )
    }
}

struct SettingsWorktrees: View {
    var worktrees: [WorktreeInfo] = []
    var error: String? = nil
    var onRefresh: () -> Void = {}

    var body: some View {
        SettingsPage(title: "Worktrees", subtitle: "CodeWith-managed worktrees returned by the app-server.") {
            refreshableListHeader(title: "Managed worktrees", onRefresh: onRefresh)
            if let error {
                statusRow(error, icon: "exclamationmark.triangle.fill", color: Theme.warning)
            } else if worktrees.isEmpty {
                statusRow("No CodeWith worktrees were returned.", icon: "folder.badge.questionmark", color: Theme.textTertiary)
            } else {
                VStack(spacing: 8) {
                    ForEach(worktrees) { worktree in
                        infoRow(
                            icon: worktree.dirty ? "folder.badge.gearshape" : "folder",
                            title: worktree.branch ?? worktree.worktreeId,
                            subtitle: [worktree.lifecycleStatus, worktree.worktreePath, worktree.dirty ? "dirty" : nil]
                                .compactMap { $0 }
                                .filter { !$0.isEmpty }
                                .joined(separator: " · "))
                    }
                }
            }
        }
    }
}

struct SettingsArchivedChats: View {
    var threads: [ThreadInfo] = []
    var error: String? = nil
    var onRefresh: () -> Void = {}
    var onUnarchive: (ThreadInfo) -> Void = { _ in }

    var body: some View {
        SettingsPage(title: "Archived chats", subtitle: "Recently archived sessions from `thread/list`.") {
            refreshableListHeader(title: "Archived sessions", onRefresh: onRefresh)
            if let error {
                statusRow(error, icon: "exclamationmark.triangle.fill", color: Theme.warning)
            } else if threads.isEmpty {
                statusRow("No archived chats were returned.", icon: "archivebox", color: Theme.textTertiary)
            } else {
                VStack(spacing: 8) {
                    ForEach(threads) { thread in
                        HStack(spacing: 10) {
                            Image(systemName: "archivebox")
                                .font(.system(size: 14))
                                .foregroundStyle(Theme.textSecondary)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(thread.name)
                                    .font(.system(size: 12.5, weight: .medium))
                                    .foregroundStyle(Theme.textPrimary)
                                    .lineLimit(1)
                                Text([thread.cwd, thread.ageLabel].compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: " · "))
                                    .font(.system(size: 11))
                                    .foregroundStyle(Theme.textSecondary)
                                    .lineLimit(1)
                            }
                            Spacer()
                            Button("Unarchive") { onUnarchive(thread) }
                                .buttonStyle(.borderless)
                        }
                        .padding(12)
                        .background(
                            RoundedRectangle(cornerRadius: 10, style: .continuous)
                                .fill(Theme.fieldFill)
                                .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(Theme.cardStroke, lineWidth: 1))
                        )
                    }
                }
            }
        }
    }
}

private func refreshableListHeader(title: String, onRefresh: @escaping () -> Void) -> some View {
    HStack {
        SettingsGroupLabel(text: title)
        Spacer()
        Button(action: onRefresh) {
            Label("Refresh", systemImage: "arrow.clockwise")
                .font(.system(size: 12, weight: .medium))
        }
        .buttonStyle(.borderless)
    }
}

private func infoRow(icon: String, title: String, subtitle: String) -> some View {
    HStack(spacing: 10) {
        Image(systemName: icon)
            .font(.system(size: 14))
            .foregroundStyle(Theme.textSecondary)
            .frame(width: 20)
        VStack(alignment: .leading, spacing: 2) {
            Text(title)
                .font(.system(size: 12.5, weight: .medium))
                .foregroundStyle(Theme.textPrimary)
                .lineLimit(1)
            Text(subtitle)
                .font(.system(size: 11))
                .foregroundStyle(Theme.textSecondary)
                .lineLimit(1)
        }
        Spacer()
    }
    .padding(12)
    .background(
        RoundedRectangle(cornerRadius: 10, style: .continuous)
            .fill(Theme.fieldFill)
            .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(Theme.cardStroke, lineWidth: 1))
    )
}

private func statusRow(_ text: String, icon: String, color: Color) -> some View {
    HStack(alignment: .top, spacing: 8) {
        Image(systemName: icon).font(.system(size: 12)).foregroundStyle(color).padding(.top, 1)
        Text(text)
            .font(.system(size: 12))
            .foregroundStyle(Theme.textSecondary)
            .fixedSize(horizontal: false, vertical: true)
        Spacer(minLength: 0)
    }
    .padding(12)
    .background(
        RoundedRectangle(cornerRadius: 8, style: .continuous)
            .fill(color.opacity(0.08))
    )
}

private func formattedCount(_ value: Int?) -> String {
    guard let value else { return "Unavailable" }
    return value.formatted()
}

private func formattedDuration(_ seconds: Int?) -> String {
    guard let seconds else { return "Unavailable" }
    let hours = seconds / 3600
    let minutes = (seconds % 3600) / 60
    if hours > 0 { return "\(hours)h \(minutes)m" }
    return "\(minutes)m"
}

private func formattedDays(_ days: Int?) -> String {
    guard let days else { return "Unavailable" }
    return days == 1 ? "1 day" : "\(days) days"
}
