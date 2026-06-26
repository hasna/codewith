import SwiftUI

/// The prompt composer. The config pills are real controls; the trailing filled
/// action swaps from microphone at rest to send/stop while active.
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

    /// Short model label to match the reference pill (e.g. "gpt-5.5-codex" → "5.5-codex").
    private var shortModel: String {
        modelLabel.hasPrefix("gpt-") ? String(modelLabel.dropFirst(4)) : modelLabel
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(alignment: .top) {
                if let text, !snapshot {
                    TextField(placeholder, text: text, axis: .vertical)
                        .textFieldStyle(.plain)
                        .font(.system(size: 13))
                        .foregroundStyle(Theme.textPrimary)
                        .lineLimit(2...6)
                        .onSubmit { onSubmit?() }
                } else {
                    // Static placeholder (also used in snapshot mode — ImageRenderer
                    // cannot render an NSTextField-backed TextField).
                    Text(text?.wrappedValue.isEmpty == false ? text!.wrappedValue : placeholder)
                        .font(.system(size: 13))
                        .foregroundStyle(text?.wrappedValue.isEmpty == false ? Theme.textPrimary : Theme.textTertiary)
                        .frame(maxWidth: .infinity, minHeight: 36, alignment: .topLeading)
                }
                Spacer()
            }
            .padding(.horizontal, 14).padding(.top, 13).padding(.bottom, 13)

            HStack(spacing: 10) {
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
                        Text(shortModel).font(.system(size: 11.5)).foregroundStyle(Theme.textSecondary).lineLimit(1)
                        Text(effortLabel).font(.system(size: 11.5)).foregroundStyle(Theme.textTertiary)
                        Image(systemName: "chevron.down").font(.system(size: 8)).foregroundStyle(Theme.textTertiary)
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain).disabled(onConfigTap == nil)

                if showSend {
                    let hasText = !(text?.wrappedValue.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ?? true)
                    let icon = stopMode ? "stop.fill" : (hasText ? "arrow.up" : "mic.fill")
                    Button { stopMode ? onStop?() : onSubmit?() } label: {
                        Circle()
                            .fill(Theme.accent)
                            .frame(width: 28, height: 28)
                            .overlay(Image(systemName: icon)
                                .font(.system(size: 10, weight: .bold)).foregroundStyle(.white))
                            .contentShape(Circle())
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(.horizontal, 12).padding(.bottom, 12)
        }
        .background(
            RoundedRectangle(cornerRadius: Theme.cardRadius, style: .continuous)
                .fill(Theme.fieldFill)
                .overlay(RoundedRectangle(cornerRadius: Theme.cardRadius, style: .continuous).strokeBorder(Theme.cardStroke, lineWidth: 1))
        )
    }
}
