import SwiftUI

/// The prompt composer. Send button, "+", and the inline config pills are all
/// real, clickable controls.
struct Composer: View {
    var placeholder: String = "Do anything"
    var showSend: Bool = true
    var stopMode: Bool = false
    var text: Binding<String>? = nil
    var onSubmit: (() -> Void)? = nil
    var onStop: (() -> Void)? = nil
    var onPlus: (() -> Void)? = nil
    var onConfigTap: (() -> Void)? = nil
    var modelLabel: String = "gpt-5.5"
    var effortLabel: String = "Low"
    @Environment(\.snapshotMode) private var snapshot

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                if let text, !snapshot {
                    TextField(placeholder, text: text, axis: .vertical)
                        .textFieldStyle(.plain)
                        .font(.system(size: 13))
                        .foregroundStyle(Theme.textPrimary)
                        .lineLimit(1...5)
                        .onSubmit { onSubmit?() }
                } else {
                    // Static placeholder (also used in snapshot mode — ImageRenderer
                    // cannot render an NSTextField-backed TextField).
                    Text(text?.wrappedValue.isEmpty == false ? text!.wrappedValue : placeholder)
                        .font(.system(size: 13))
                        .foregroundStyle(text?.wrappedValue.isEmpty == false ? Theme.textPrimary : Theme.textTertiary)
                }
                Spacer()
            }
            .padding(.horizontal, 14).padding(.top, 12).padding(.bottom, 18)

            HStack(spacing: 10) {
                Button { onPlus?() } label: {
                    Image(systemName: "plus").font(.system(size: 13, weight: .regular)).foregroundStyle(Theme.textTertiary)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain).disabled(onPlus == nil)

                // Full access pill — subtle; opens the in-session config panel.
                Button { onConfigTap?() } label: {
                    HStack(spacing: 4) {
                        Text("Full access").font(.system(size: 11.5, weight: .regular))
                        Image(systemName: "chevron.down").font(.system(size: 8))
                    }
                    .foregroundStyle(Theme.textSecondary).contentShape(Rectangle())
                }
                .buttonStyle(.plain).disabled(onConfigTap == nil)

                Spacer()

                // Model + effort pill — opens the config panel.
                Button { onConfigTap?() } label: {
                    HStack(spacing: 3) {
                        Text(modelLabel).font(.system(size: 11.5)).foregroundStyle(Theme.textSecondary).lineLimit(1)
                        Text(effortLabel).font(.system(size: 11.5)).foregroundStyle(Theme.textTertiary)
                        Image(systemName: "chevron.down").font(.system(size: 8)).foregroundStyle(Theme.textTertiary)
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain).disabled(onConfigTap == nil)

                Image(systemName: "mic").font(.system(size: 12)).foregroundStyle(Theme.textSecondary)

                if showSend {
                    // Active (black) while typing or running; gray only when empty/idle.
                    let hasText = !(text?.wrappedValue.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ?? true)
                    let active = stopMode || hasText
                    Button { stopMode ? onStop?() : onSubmit?() } label: {
                        Circle()
                            .fill(active ? Color(hex: 0x202020) : Color(hex: 0xBEBEBE))
                            .frame(width: 24, height: 24)
                            .overlay(Image(systemName: stopMode ? "stop.fill" : "arrow.up")
                                .font(.system(size: 10, weight: .bold)).foregroundStyle(.white))
                            .contentShape(Circle())
                    }
                    .buttonStyle(.plain)
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
