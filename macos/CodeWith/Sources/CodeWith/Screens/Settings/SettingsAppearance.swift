import SwiftUI

struct SettingsAppearance: View {
    var body: some View {
        SettingsPage(title: "Appearance") {
            VStack(alignment: .leading, spacing: 0) {
                // Theme cards
                HStack(spacing: 14) {
                    themeCard(title: "System", selected: true, mode: .system)
                    themeCard(title: "Light", selected: false, mode: .light)
                    themeCard(title: "Dark", selected: false, mode: .dark)
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

    enum ThemeMode { case system, light, dark }

    private func themeCard(title: String, selected: Bool, mode: ThemeMode) -> some View {
        VStack(spacing: 8) {
            ZStack {
                switch mode {
                case .light: miniApp(dark: false)
                case .dark:  miniApp(dark: true)
                case .system:
                    HStack(spacing: 0) {
                        miniApp(dark: false).frame(maxWidth: .infinity).clipped()
                        miniApp(dark: true).frame(maxWidth: .infinity).clipped()
                    }
                }
            }
            .frame(height: 80)
            .clipShape(RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(selected ? Theme.toggleBlue : Theme.cardStroke, lineWidth: selected ? 2 : 1))
            Text(title).font(.system(size: 11.5)).foregroundStyle(Theme.textSecondary)
        }
        .frame(maxWidth: .infinity)
    }

    /// A tiny app-window mockup: sidebar (with rows) + content (title + lines).
    private func miniApp(dark: Bool) -> some View {
        let bg = dark ? Color(hex: 0x1F2123) : Color.white
        let side = dark ? Color(hex: 0x2A2C2F) : Color(hex: 0xF1F1F3)
        let ink = (dark ? Color.white : Color.black)
        return HStack(spacing: 0) {
            // sidebar
            VStack(alignment: .leading, spacing: 4) {
                Capsule().fill(Color(hex: 0x6E6BF2)).frame(width: 14, height: 4)
                ForEach(0..<4, id: \.self) { _ in
                    Capsule().fill(ink.opacity(0.16)).frame(width: 22, height: 3)
                }
                Spacer()
            }
            .padding(6)
            .frame(width: 40)
            .background(side)
            // content
            VStack(alignment: .leading, spacing: 5) {
                Capsule().fill(ink.opacity(0.30)).frame(width: 40, height: 5)
                ForEach(0..<4, id: \.self) { i in
                    Capsule().fill(ink.opacity(0.12)).frame(width: i == 3 ? 30 : 52, height: 3)
                }
                Spacer()
            }
            .padding(7)
            .frame(maxWidth: .infinity, alignment: .topLeading)
            .background(bg)
        }
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
