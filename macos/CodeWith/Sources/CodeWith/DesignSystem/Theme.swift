import SwiftUI

/// Design tokens for CodeWith — a Codex-parity macOS app using the macOS 26
/// "Liquid Glass" design language. Values are tuned against the reference
/// screenshots (light theme).
enum Theme {
    // MARK: Palette (light)
    /// App window background — the main detail canvas is pure white.
    static let canvas = Color.white
    /// Sidebar background — a faint translucent gray over the desktop glass.
    static let sidebar = Color(nsColor: NSColor(calibratedWhite: 0.96, alpha: 1.0))
    /// Hairline separators.
    static let separator = Color(nsColor: NSColor(calibratedWhite: 0.0, alpha: 0.08))
    /// Primary text.
    static let textPrimary = Color(nsColor: NSColor(calibratedWhite: 0.10, alpha: 1.0))
    /// Secondary / muted text.
    static let textSecondary = Color(nsColor: NSColor(calibratedWhite: 0.42, alpha: 1.0))
    /// Tertiary / very muted (timestamps, placeholders).
    static let textTertiary = Color(nsColor: NSColor(calibratedWhite: 0.62, alpha: 1.0))
    /// Accent — the indigo/violet used by the brand mark & primary actions.
    static let accent = Color(red: 0.36, green: 0.34, blue: 0.92)
    /// Toggle / control "on" blue (Codex uses Apple's system blue, not the brand violet).
    static let toggleBlue = Color(hex: 0x0A84FF)
    /// Warning red-orange used by "Full access" (sampled from the reference, #D3642F).
    static let warning = Color(hex: 0xD3642F)
    /// Success green.
    static let success = Color(red: 0.18, green: 0.62, blue: 0.34)
    /// Danger red.
    static let danger = Color(red: 0.84, green: 0.22, blue: 0.20)

    /// Hover / selected row fill in the sidebar.
    static let rowSelected = Color(nsColor: NSColor(calibratedWhite: 0.0, alpha: 0.06))
    static let rowHover = Color(nsColor: NSColor(calibratedWhite: 0.0, alpha: 0.035))

    /// Subtle field/card fills.
    static let fieldFill = Color(nsColor: NSColor(calibratedWhite: 0.97, alpha: 1.0))
    static let cardStroke = Color(nsColor: NSColor(calibratedWhite: 0.0, alpha: 0.07))

    // MARK: Typography
    static func font(_ size: CGFloat, _ weight: Font.Weight = .regular) -> Font {
        .system(size: size, weight: weight)
    }
    static let sidebarItem = Font.system(size: 12.5, weight: .regular)
    static let sidebarSection = Font.system(size: 11, weight: .semibold)
    static let title = Font.system(size: 22, weight: .regular)
    static let body = Font.system(size: 13, weight: .regular)
    static let small = Font.system(size: 11.5, weight: .regular)

    // MARK: Metrics
    static let sidebarWidth: CGFloat = 215
    static let rowRadius: CGFloat = 7
    static let cardRadius: CGFloat = 12
    static let windowRadius: CGFloat = 12
}

extension Color {
    init(hex: UInt32, alpha: Double = 1.0) {
        let r = Double((hex >> 16) & 0xFF) / 255.0
        let g = Double((hex >> 8) & 0xFF) / 255.0
        let b = Double(hex & 0xFF) / 255.0
        self = Color(.sRGB, red: r, green: g, blue: b, opacity: alpha)
    }
}
