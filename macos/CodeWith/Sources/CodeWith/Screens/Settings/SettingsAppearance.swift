import SwiftUI

struct SettingsAppearance: View {
    var body: some View {
        SettingsPage(title: "Appearance") {
            VStack(alignment: .leading, spacing: 0) {
                // Theme cards
                HStack(spacing: 14) {
                    themeCard(title: "System", selected: true, left: Color(hex: 0xC9CDD2), right: Color(hex: 0x2B2D30))
                    themeCard(title: "Light", selected: false, left: Color(hex: 0xF3F3F5), right: Color(hex: 0xF3F3F5))
                    themeCard(title: "Dark", selected: false, left: Color(hex: 0x2B2D30), right: Color(hex: 0x2B2D30))
                }
                .padding(.bottom, 16)

                // themeConfig diff
                HStack(spacing: 0) {
                    codeBlock(lines: [
                        ("1", "const themePreview: ThemeConfig = {", .plain),
                        ("2", "  surface: \"sidebar\",", .del),
                        ("3", "  accent: \"#2563eb\",", .del),
                        ("4", "  contrast: 42,", .del),
                        ("5", "};", .plain),
                    ], side: .del)
                    codeBlock(lines: [
                        ("1", "const themePreview: ThemeConfig = {", .plain),
                        ("2", "  surface: \"sidebar-elevated\",", .add),
                        ("3", "  accent: \"#0ea5e9\",", .add),
                        ("4", "  contrast: 68,", .add),
                        ("5", "};", .plain),
                    ], side: .add)
                }
                .clipShape(RoundedRectangle(cornerRadius: 8))
                .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Theme.cardStroke, lineWidth: 1))
                .padding(.bottom, 22)

                themeEditor(title: "Light theme", bg: "#FFFFFF", bgColor: .white, fg: "#1A1C1F", fgColor: Color(hex: 0x1A1C1F))
                Rectangle().fill(Theme.separator).frame(height: 1).padding(.vertical, 14)
                themeEditor(title: "Dark theme", bg: "#181818", bgColor: Color(hex: 0x181818), fg: "#FFFFFF", fgColor: .white)
            }
        }
    }

    private func themeCard(title: String, selected: Bool, left: Color, right: Color) -> some View {
        VStack(spacing: 8) {
            HStack(spacing: 0) {
                left.overlay(alignment: .topLeading) { miniLines(dark: false) }
                right.overlay(alignment: .topLeading) { miniLines(dark: true) }
            }
            .frame(height: 78)
            .clipShape(RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(selected ? Theme.toggleBlue : Theme.cardStroke, lineWidth: selected ? 2 : 1))
            Text(title).font(.system(size: 11.5)).foregroundStyle(Theme.textSecondary)
        }
        .frame(maxWidth: .infinity)
    }
    private func miniLines(dark: Bool) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            ForEach(0..<3, id: \.self) { _ in
                Capsule().fill((dark ? Color.white : Color.black).opacity(0.18)).frame(width: 34, height: 3)
            }
        }
        .padding(8)
    }

    enum DiffKind { case plain, add, del }
    private func codeBlock(lines: [(String, String, DiffKind)], side: DiffKind) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(lines, id: \.0) { ln in
                HStack(spacing: 8) {
                    Text(ln.0).font(.system(size: 9.5, design: .monospaced)).foregroundStyle(Theme.textTertiary).frame(width: 14, alignment: .trailing)
                    Text(ln.1).font(.system(size: 9.5, design: .monospaced)).foregroundStyle(Theme.textPrimary)
                    Spacer()
                }
                .padding(.horizontal, 8).padding(.vertical, 2)
                .background(ln.2 == .del ? Color(hex: 0xFDECEC) : (ln.2 == .add ? Color(hex: 0xE6F6EC) : Color.clear))
            }
        }
        .padding(.vertical, 6)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(hex: 0xFBFBFB))
    }

    private func themeEditor(title: String, bg: String, bgColor: Color, fg: String, fgColor: Color) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text(title).font(.system(size: 12.5, weight: .medium)).foregroundStyle(Theme.textPrimary)
                Spacer()
                HStack(spacing: 12) {
                    Text("Import").font(.system(size: 11)).foregroundStyle(Theme.textSecondary)
                    Text("Copy theme").font(.system(size: 11)).foregroundStyle(Theme.textSecondary)
                    DropdownPill(text: "CodeWith", icon: "textformat")
                }
            }
            .padding(.bottom, 8)
            colorRow("Accent", "#339CFF", Color(hex: 0x339CFF))
            colorRow("Background", bg, bgColor)
            colorRow("Foreground", fg, fgColor)
            fieldRow("UI font", "-apple-system, Blink")
            fieldRow("Code font", "ui-monospace, \"SFM")
            SettingsRow(title: "Translucent sidebar", showDivider: false) { GlassToggle(on: true) }
            HStack {
                Text("Contrast").font(.system(size: 13)).foregroundStyle(Theme.textPrimary)
                Spacer()
                Capsule().fill(Color(hex: 0xD8D8DC)).frame(width: 160, height: 4)
                    .overlay(alignment: .leading) { Circle().fill(.white).frame(width: 14, height: 14).overlay(Circle().strokeBorder(Theme.cardStroke)).offset(x: 80) }
                Text("45").font(.system(size: 12)).foregroundStyle(Theme.textSecondary).frame(width: 22)
            }
            .padding(.vertical, 7)
        }
    }
    private func colorRow(_ label: String, _ hex: String, _ color: Color) -> some View {
        HStack {
            Text(label).font(.system(size: 13)).foregroundStyle(Theme.textPrimary)
            Spacer()
            HStack(spacing: 8) {
                Text(hex).font(.system(size: 11, design: .monospaced)).foregroundStyle(color == .white ? Theme.textSecondary : .white)
            }
            .padding(.horizontal, 10).frame(width: 110, height: 24)
            .background(RoundedRectangle(cornerRadius: 6).fill(color).overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(Theme.cardStroke, lineWidth: 1)))
        }
        .padding(.vertical, 6)
        .overlay(alignment: .bottom) { Rectangle().fill(Theme.separator).frame(height: 1) }
    }
    private func fieldRow(_ label: String, _ value: String) -> some View {
        HStack {
            Text(label).font(.system(size: 13)).foregroundStyle(Theme.textPrimary)
            Spacer()
            Text(value).font(.system(size: 11, design: .monospaced)).foregroundStyle(Theme.textTertiary)
        }
        .padding(.vertical, 8)
        .overlay(alignment: .bottom) { Rectangle().fill(Theme.separator).frame(height: 1) }
    }
}
