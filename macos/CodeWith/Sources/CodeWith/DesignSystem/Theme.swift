import SwiftUI

/// Design tokens for Codewith, mapped to the neutral Hasna/Open dashboard
/// references found locally: white card surfaces, near-black primary controls,
/// low-contrast gray chrome, and 10px-radius controls.
enum Theme {
    // MARK: Palette (light)
    static let canvas = Color.white
    static let sidebar = Color(hex: 0xFAFAFA)
    static let separator = Color(hex: 0xE5E5E5)
    static let textPrimary = Color(hex: 0x171717)
    static let textSecondary = Color(hex: 0x737373)
    static let textTertiary = Color(hex: 0xA3A3A3)
    static let accent = Color(hex: 0x0A0A0A)
    static let toggleBlue = Color(hex: 0x0A0A0A)
    static let warning = Color(hex: 0xF97316)
    static let success = Color(hex: 0x22C55E)
    static let danger = Color(hex: 0xEF4444)

    static let rowSelected = Color(hex: 0xF5F5F5)
    static let rowHover = Color(hex: 0xF5F5F5)

    static let fieldFill = Color.white
    static let controlFill = Color(hex: 0xF5F5F5)
    static let cardStroke = Color(hex: 0xE5E5E5)

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
    static let rowRadius: CGFloat = 8
    static let cardRadius: CGFloat = 10
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
