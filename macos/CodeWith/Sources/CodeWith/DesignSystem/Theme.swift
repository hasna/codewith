import SwiftUI

/// Design tokens for CodeWith, mapped from the Hasna Nopen dashboard reference
/// on spark02: warm OKLCH neutrals, teal primary controls, and rounded shadcn
/// surfaces.
enum Theme {
    // MARK: Palette (light)
    static let canvas = Color(hex: 0xFFFFFF)
    static let sidebar = Color(hex: 0xFBFBF9)
    static let separator = Color(hex: 0xE8E8E3)
    static let textPrimary = Color(hex: 0x0C0C09)
    static let textSecondary = Color(hex: 0x7C7C67)
    static let textTertiary = Color(hex: 0xABAB9C)
    static let accent = Color(hex: 0x007595)
    static let accentHover = Color(hex: 0x0092B8)
    static let accentForeground = Color(hex: 0xECFEFF)
    static let toggleBlue = Color(hex: 0x007595)
    static let warning = Color(hex: 0x7C7C67)
    static let success = Color(hex: 0x0092B8)
    static let danger = Color(hex: 0xE7000B)

    static let rowSelected = Color(hex: 0xF4F4F0)
    static let rowHover = Color(hex: 0xF4F4F0)

    static let fieldFill = Color(hex: 0xFFFFFF)
    static let controlFill = Color(hex: 0xF4F4F0)
    static let cardStroke = Color(hex: 0xE8E8E3)

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
    static let rowRadius: CGFloat = 11
    static let cardRadius: CGFloat = 20
    static let windowRadius: CGFloat = 14
}

extension Color {
    init(hex: UInt32, alpha: Double = 1.0) {
        let r = Double((hex >> 16) & 0xFF) / 255.0
        let g = Double((hex >> 8) & 0xFF) / 255.0
        let b = Double(hex & 0xFF) / 255.0
        self = Color(.sRGB, red: r, green: g, blue: b, opacity: alpha)
    }
}
