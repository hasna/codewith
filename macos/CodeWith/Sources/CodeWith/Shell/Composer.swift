import SwiftUI

/// The prompt composer used on Home and Chat screens.
struct Composer: View {
    var placeholder: String = "Do anything"
    var showSend: Bool = true
    var stopMode: Bool = false
    var text: Binding<String>? = nil
    var onSubmit: (() -> Void)? = nil
    var onPlus: (() -> Void)? = nil

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                if let text {
                    TextField(placeholder, text: text, axis: .vertical)
                        .textFieldStyle(.plain)
                        .font(.system(size: 13))
                        .foregroundStyle(Theme.textPrimary)
                        .lineLimit(1...5)
                        .onSubmit { onSubmit?() }
                } else {
                    Text(placeholder).font(.system(size: 13)).foregroundStyle(Theme.textTertiary)
                }
                Spacer()
            }
            .padding(.horizontal, 14).padding(.top, 12).padding(.bottom, 18)

            HStack(spacing: 10) {
                Button { onPlus?() } label: {
                    Image(systemName: "plus").font(.system(size: 13, weight: .regular)).foregroundStyle(Theme.textTertiary)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .disabled(onPlus == nil)
                // Full access pill
                HStack(spacing: 4) {
                    Image(systemName: "exclamationmark.triangle.fill").font(.system(size: 9))
                    Text("Full access").font(.system(size: 11.5, weight: .medium))
                    Image(systemName: "chevron.down").font(.system(size: 8))
                }
                .foregroundStyle(Theme.warning)
                Spacer()
                HStack(spacing: 3) {
                    Text("5.5").font(.system(size: 11.5)).foregroundStyle(Theme.textSecondary)
                    Text("Low").font(.system(size: 11.5)).foregroundStyle(Theme.textTertiary)
                    Image(systemName: "chevron.down").font(.system(size: 8)).foregroundStyle(Theme.textTertiary)
                }
                Image(systemName: "mic").font(.system(size: 12)).foregroundStyle(Theme.textSecondary)
                if showSend {
                    Circle()
                        .fill(stopMode ? Color(hex: 0x202020) : Color(hex: 0xBEBEBE))
                        .frame(width: 24, height: 24)
                        .overlay(
                            Image(systemName: stopMode ? "stop.fill" : "arrow.up")
                                .font(.system(size: 10, weight: .bold)).foregroundStyle(.white)
                        )
                }
            }
            .padding(.horizontal, 12).padding(.bottom, 10)
        }
        .background(
            RoundedRectangle(cornerRadius: 14, style: .continuous)
                .fill(Theme.fieldFill)
                .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Theme.cardStroke, lineWidth: 1))
        )
    }
}
