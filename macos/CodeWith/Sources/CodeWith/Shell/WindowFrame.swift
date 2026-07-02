import SwiftUI

/// Draws the macOS window chrome (rounded corners, drop shadow, traffic-light
/// buttons) around content so snapshots match the reference window captures.
struct WindowFrame<Content: View>: View {
    var showTrafficLights: Bool = true
    @ViewBuilder var content: () -> Content

    var body: some View {
        content()
            .frame(width: WindowSize.app.width, height: WindowSize.app.height, alignment: .top)
            .clipShape(RoundedRectangle(cornerRadius: Theme.windowRadius, style: .continuous))
            .overlay(alignment: .topLeading) {
                if showTrafficLights {
                    TrafficLights().padding(.leading, 13).padding(.top, 13)
                }
            }
            .overlay(
                RoundedRectangle(cornerRadius: Theme.windowRadius, style: .continuous)
                    .strokeBorder(Theme.cardStroke, lineWidth: 0.5)
            )
            .shadow(color: .black.opacity(0.12), radius: 18, x: 0, y: 10)
            .padding(22)
            .background(Theme.controlFill)
    }
}

struct TrafficLights: View {
    var body: some View {
        HStack(spacing: 8) {
            dot(Color(hex: 0xFF5F57))
            dot(Color(hex: 0xFEBC2E))
            dot(Color(hex: 0x28C840))
        }
    }
    private func dot(_ c: Color) -> some View {
        Circle()
            .fill(c)
            .frame(width: 12, height: 12)
            .overlay(Circle().strokeBorder(Color.black.opacity(0.06), lineWidth: 0.5))
    }
}
