import SwiftUI

struct SettingsProfile: View {
    var account: AccountInfo? = nil

    private let stats: [(String, String)] = [
        ("9.3B", "Lifetime tokens"), ("2.2B", "Peak tokens"), ("5h 27m", "Longest task"),
        ("8 days", "Current streak"), ("8 days", "Longest streak"),
    ]
    private let insights: [(String, String)] = [
        ("Fast Mode", "44%"), ("Most used reasoning", "Extra High · 47%"),
        ("Skills explored", "77"), ("Total skills used", "809"), ("Total threads", "298"),
    ]
    private let plugins: [(String, String)] = [
        ("$skill-ai-runtime-streaming", "134 runs"), ("$skill-tenant-security-audit", "111 runs"),
        ("$open-loops-daemon", "39 runs"), ("$open-loops-cli", "38 runs"),
        ("$skill-admin-implementation", "33 runs"),
    ]

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            // Top bar: Profile + actions
            HStack {
                Text("Profile").font(.system(size: 13)).foregroundStyle(Theme.textSecondary)
                Spacer()
                topAction("square.and.arrow.up", "Share")
                topAction("lock", "Private")
                topAction("pencil", "Edit")
            }
            .padding(.horizontal, 22).frame(height: 40)

            VStack(spacing: 0) {
                Circle().fill(Color(hex: 0x4AB58E))
                    .frame(width: 64, height: 64)
                    .overlay(Text(account?.initials ?? "ME").font(.system(size: 22, weight: .semibold)).foregroundStyle(.white))
                    .padding(.top, 26).padding(.bottom, 12)
                Text(account?.name ?? "Signed out").font(.system(size: 18, weight: .semibold)).foregroundStyle(Theme.textPrimary)
                HStack(spacing: 6) {
                    if let email = account?.email, !email.isEmpty {
                        Text(email).font(.system(size: 12)).foregroundStyle(Theme.textSecondary)
                    }
                    if let plan = account?.plan, !plan.isEmpty {
                        Text(plan).font(.system(size: 11)).foregroundStyle(Theme.textTertiary)
                    }
                }
                .padding(.top, 3).padding(.bottom, 18)

                // Stats strip
                HStack(spacing: 0) {
                    ForEach(Array(stats.enumerated()), id: \.0) { i, s in
                        VStack(spacing: 4) {
                            Text(s.0).font(.system(size: 15, weight: .semibold)).foregroundStyle(Theme.textPrimary)
                            Text(s.1).font(.system(size: 10.5)).foregroundStyle(Theme.textSecondary)
                        }
                        .frame(maxWidth: .infinity)
                        if i < stats.count - 1 { Rectangle().fill(Theme.separator).frame(width: 1, height: 30) }
                    }
                }
                .padding(.vertical, 12)
                .background(RoundedRectangle(cornerRadius: 10).strokeBorder(Theme.cardStroke, lineWidth: 1))
                .padding(.horizontal, 40)
            }

            // Token activity
            HStack {
                Text("Token activity").font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.textPrimary)
                Spacer()
                HStack(spacing: 12) {
                    Text("Daily").font(.system(size: 11)).foregroundStyle(Theme.textPrimary)
                    Text("Weekly").font(.system(size: 11)).foregroundStyle(Theme.textTertiary)
                    Text("Cumulative").font(.system(size: 11)).foregroundStyle(Theme.textTertiary)
                }
            }
            .padding(.horizontal, 40).padding(.top, 24).padding(.bottom, 10)
            Heatmap().padding(.horizontal, 40)

            // Insights + plugins
            HStack(alignment: .top, spacing: 40) {
                VStack(alignment: .leading, spacing: 0) {
                    Text("Activity insights").font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.textPrimary).padding(.bottom, 8)
                    ForEach(insights, id: \.0) { row in
                        HStack { Text(row.0).font(.system(size: 12)).foregroundStyle(Theme.textSecondary); Spacer(); Text(row.1).font(.system(size: 12)).foregroundStyle(Theme.textPrimary) }
                            .padding(.vertical, 5)
                    }
                }
                VStack(alignment: .leading, spacing: 0) {
                    Text("Most used plugins").font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.textPrimary).padding(.bottom, 8)
                    ForEach(Array(plugins.enumerated()), id: \.0) { i, row in
                        HStack(spacing: 8) {
                            Circle().fill([Color(hex: 0xE9A23B), Color(hex: 0x4AB58E), Color(hex: 0x6E6BF2), Color(hex: 0xDB5B5B), Color(hex: 0x3B82F6)][i % 5]).frame(width: 14, height: 14)
                            Text(row.0).font(.system(size: 12)).foregroundStyle(Theme.textPrimary)
                            Spacer()
                            Text(row.1).font(.system(size: 12)).foregroundStyle(Theme.textSecondary)
                        }
                        .padding(.vertical, 5)
                    }
                }
            }
            .padding(.horizontal, 40).padding(.top, 24)
            Spacer(minLength: 20)
        }
    }

    private func topAction(_ icon: String, _ label: String) -> some View {
        HStack(spacing: 4) {
            Image(systemName: icon).font(.system(size: 10))
            Text(label).font(.system(size: 11.5))
        }
        .foregroundStyle(Theme.textSecondary).padding(.leading, 14)
    }
}

struct Heatmap: View {
    let cols = 26, rows = 7
    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(alignment: .top, spacing: 3) {
                ForEach(0..<cols, id: \.self) { c in
                    VStack(spacing: 3) {
                        ForEach(0..<rows, id: \.self) { r in
                            let v = intensity(c, r)
                            RoundedRectangle(cornerRadius: 2)
                                .fill(v == 0 ? Color(hex: 0xEFF1F4) : Color(hex: 0xDCE3F8).mix(with: Color(hex: 0x5B8DEF), by: v))
                                .frame(width: 16, height: 11)
                        }
                    }
                }
            }
            HStack(spacing: 0) {
                ForEach(["Jul","Aug","Sep","Oct","Nov","Dec","Jan","Feb","Mar","Apr","May","Jun"], id: \.self) {
                    Text($0).font(.system(size: 9)).foregroundStyle(Theme.textTertiary).frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            .padding(.top, 2)
        }
    }
    private func intensity(_ c: Int, _ r: Int) -> Double {
        // Activity only in the last ~4 columns (Apr–Jun); everything before is empty.
        guard c >= 22 else { return 0 }
        let seed = (c * 7 + r * 5) % 4
        return seed == 0 ? 0 : Double(seed) / 3.0
    }
}
