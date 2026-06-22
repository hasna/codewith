import SwiftUI

struct SettingsPage<Content: View>: View {
    var title: String
    var subtitle: String? = nil
    @ViewBuilder var content: () -> Content
    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(title).font(.system(size: 21, weight: .semibold)).foregroundStyle(Theme.textPrimary)
                .padding(.top, 22)
            if let s = subtitle {
                Text(s).font(.system(size: 12)).foregroundStyle(Theme.textSecondary).padding(.top, 4)
            }
            content().padding(.top, 16)
            Spacer(minLength: 20)
        }
        .padding(.horizontal, 40)
        .frame(maxWidth: 680, alignment: .leading)        // constrain content like the reference (no full-bleed)
        .frame(maxWidth: .infinity, alignment: .leading)  // …and sit at the left of the pane
    }
}

struct SettingsGroupLabel: View {
    var text: String
    var body: some View {
        Text(text).font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.textPrimary)
            .padding(.bottom, 10)
    }
}

/// A titled row with an explanatory subtitle on the left and a trailing control.
struct SettingsRow<Trailing: View>: View {
    var title: String
    var subtitle: String? = nil
    var showDivider: Bool = true
    @ViewBuilder var trailing: () -> Trailing
    var body: some View {
        VStack(spacing: 0) {
            HStack(alignment: .center, spacing: 16) {
                VStack(alignment: .leading, spacing: 3) {
                    Text(title).font(.system(size: 13)).foregroundStyle(Theme.textPrimary)
                    if let s = subtitle {
                        Text(s).font(.system(size: 11.5)).foregroundStyle(Theme.textSecondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                Spacer()
                trailing()
            }
            .padding(.vertical, 7)
            if showDivider { Rectangle().fill(Theme.separator).frame(height: 1) }
        }
    }
}

struct GlassToggle: View {
    var on: Bool
    var onTap: (() -> Void)? = nil

    @ViewBuilder private var capsule: some View {
        Capsule()
            .fill(on ? Theme.toggleBlue : Color(hex: 0xD8D8DC))
            .frame(width: 28, height: 16)
            .overlay(alignment: on ? .trailing : .leading) {
                Circle().fill(.white).frame(width: 12, height: 12).padding(2)
                    .shadow(color: .black.opacity(0.15), radius: 1, y: 1)
            }
    }
    var body: some View {
        if let onTap {
            Button(action: onTap) { capsule.contentShape(Capsule()) }.buttonStyle(.plain)
        } else {
            capsule
        }
    }
}

struct DropdownPill: View {
    var text: String
    var icon: String? = nil
    var body: some View {
        HStack(spacing: 6) {
            if let icon { Image(systemName: icon).font(.system(size: 11)) }
            Text(text).font(.system(size: 12))
            Image(systemName: "chevron.up.chevron.down").font(.system(size: 8)).foregroundStyle(Theme.textTertiary)
        }
        .foregroundStyle(Theme.textPrimary)
        .padding(.horizontal, 10).frame(height: 28)
        .background(RoundedRectangle(cornerRadius: 7).fill(Theme.fieldFill)
            .overlay(RoundedRectangle(cornerRadius: 7).strokeBorder(Theme.cardStroke, lineWidth: 1)))
    }
}

/// The two-up "work mode" choice cards.
struct ChoiceCard: View {
    var icon: String
    var title: String
    var subtitle: String
    var selected: Bool
    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: icon).font(.system(size: 14)).foregroundStyle(Theme.textSecondary)
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.system(size: 12.5, weight: .medium)).foregroundStyle(Theme.textPrimary)
                Text(subtitle).font(.system(size: 11)).foregroundStyle(Theme.textSecondary)
            }
            Spacer()
            ZStack {
                if selected {
                    Circle().fill(Theme.accent).frame(width: 16, height: 16)
                    Circle().fill(.white).frame(width: 6, height: 6)
                } else {
                    Circle().strokeBorder(Theme.cardStroke, lineWidth: 1).frame(width: 16, height: 16)
                }
            }
        }
        .padding(.horizontal, 12).padding(.vertical, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 10).fill(Color.white)
            .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(selected ? Theme.toggleBlue.opacity(0.5) : Theme.cardStroke, lineWidth: 1)))
    }
}
