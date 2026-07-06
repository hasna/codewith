import SwiftUI

/// The fork's "Apps" gallery (replaces Codex "Plugins"). A searchable grid of
/// app/skill cards drawn from the hasna ecosystem.
struct AppsView: View {
    var apps: [AppItemInfo] = []

    private let columns = Array(repeating: GridItem(.flexible(), spacing: 12), count: 3)
    private let tints: [Color] = [
        Theme.accent, Theme.textSecondary, Theme.warning, Theme.success, Theme.danger,
    ]

    var body: some View {
        VStack(spacing: 0) {
            // Detail top bar — matches the reference/settings pattern.
            HStack {
                Text("Apps").font(.system(size: 13)).foregroundStyle(Theme.textSecondary)
                Spacer()
            }
            .padding(.horizontal, 22).frame(height: 40)
            Rectangle().fill(Theme.separator).frame(height: 1)

            ScrollColumn(spacing: 0) {
                if apps.isEmpty {
                    Text("No apps available.")
                        .font(.system(size: 12)).foregroundStyle(Theme.textTertiary)
                        .padding(.horizontal, 24).padding(.vertical, 20)
                } else {
                    LazyVGrid(columns: columns, spacing: 12) {
                        ForEach(Array(apps.enumerated()), id: \.element.id) { i, app in
                            card(app, tint: tints[i % tints.count])
                        }
                    }
                    .padding(.horizontal, 24)
                    .padding(.vertical, 20)
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.canvas)
    }

    /// Pick a representative SF Symbol from the app's name/description keywords.
    private func icon(for app: AppItemInfo) -> String {
        let n = (app.name + " " + app.detail).lowercased()
        // Order matters: check specific/safe keywords before looser ones so e.g.
        // "reports" doesn't get matched as a git "repo".
        switch true {
        case n.contains("mail"), n.contains("email"), n.contains("inbox"): return "envelope"
        case n.contains("search"), n.contains("research"), n.contains("index"): return "magnifyingglass"
        case n.contains("security"), n.contains("audit"), n.contains("secret"), n.contains("vault"): return "lock.shield"
        case n.contains("github"), n.contains("gitlab"), n.contains("repository"), n.contains("branch"), n.contains("commit"): return "arrow.triangle.branch"
        case n.contains("loop"), n.contains("schedule"), n.contains("cron"), n.contains("daemon"): return "arrow.trianglehead.2.clockwise.rotate.90"
        case n.contains("deploy"), n.contains("ship"), n.contains("release"), n.contains("publish"): return "shippingbox"
        case n.contains("test"): return "checkmark.seal"
        case n.contains("database"), n.contains("postgres"), n.contains("sql"): return "cylinder.split.1x2"
        case n.contains("chat"), n.contains("message"), n.contains("slack"), n.contains("whatsapp"): return "bubble.left.and.bubble.right"
        case n.contains("memo"), n.contains("note"), n.contains("document"): return "doc.text"
        case n.contains("image"), n.contains("photo"), n.contains("design"): return "photo"
        case n.contains("web"), n.contains("http"), n.contains("browser"), n.contains("scrape"): return "globe"
        case n.contains("calendar"), n.contains("event"): return "calendar"
        case n.contains("voice"), n.contains("audio"), n.contains("phone"), n.contains("call"), n.contains("record"): return "waveform"
        case n.contains("task"), n.contains("todo"), n.contains("plan"): return "checklist"
        default: return "square.grid.2x2.fill"
        }
    }

    private func card(_ app: AppItemInfo, tint: Color) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            RoundedRectangle(cornerRadius: 9, style: .continuous)
                .fill(tint.opacity(0.14))
                .frame(width: 36, height: 36)
                .overlay(Image(systemName: icon(for: app)).font(.system(size: 15)).foregroundStyle(tint))
            Text(app.name).font(.system(size: 12.5, weight: .semibold)).foregroundStyle(Theme.textPrimary).lineLimit(1)
            Text(app.detail).font(.system(size: 11)).foregroundStyle(Theme.textSecondary)
                .lineLimit(2).fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 6)
            installPill(installed: app.enabled)
        }
        .padding(12)
        .frame(height: 138, alignment: .topLeading)
        .frame(maxWidth: .infinity, alignment: .topLeading)
        .background(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .fill(Theme.fieldFill)
                .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Theme.cardStroke, lineWidth: 1))
        )
    }

    private func installPill(installed: Bool) -> some View {
        HStack(spacing: 4) {
            if installed {
                Image(systemName: "checkmark").font(.system(size: 9, weight: .semibold))
            }
            Text(installed ? "Installed" : "Available").font(.system(size: 11, weight: .medium))
        }
        .foregroundStyle(installed ? Theme.textSecondary : Theme.textPrimary)
        .padding(.horizontal, 12).frame(height: 24)
        .background(
            Capsule().fill(Theme.fieldFill)
                .overlay(Capsule().strokeBorder(Theme.cardStroke, lineWidth: 1))
        )
    }
}
