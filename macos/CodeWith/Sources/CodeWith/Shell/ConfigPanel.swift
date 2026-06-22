import SwiftUI

/// The in-session configuration right sidebar: switch model / gateway / effort
/// for the current session without leaving it. Opened by the top-right button.
struct ConfigPanel: View {
    @Bindable var model: AppModel

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text("Session").font(.system(size: 12, weight: .medium)).foregroundStyle(Theme.textPrimary)
                Spacer()
                Button { model.showConfigPanel = false } label: {
                    Image(systemName: "xmark").font(.system(size: 11)).foregroundStyle(Theme.textTertiary)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
            .padding(.horizontal, 14).frame(height: 40)
            Rectangle().fill(Theme.separator).frame(height: 1)

            ScrollColumn(alignment: .leading, spacing: 14) {
                pickerRow("Model", value: model.model ?? "default", options: model.availableModels) { model.setModel($0) }
                pickerRow("Gateway", value: model.provider ?? "openai", options: model.availableProviders) { model.setProvider($0) }
                pickerRow("Effort", value: model.effort, options: model.availableEfforts) { model.setEffort($0) }

                Rectangle().fill(Theme.separator).frame(height: 1).padding(.vertical, 2)

                HStack(spacing: 4) {
                    Image(systemName: "exclamationmark.triangle.fill").font(.system(size: 10)).foregroundStyle(Theme.warning)
                    Text("Full access").font(.system(size: 12, weight: .medium)).foregroundStyle(Theme.warning)
                    Spacer()
                    GlassToggle(on: model.fullAccess) { model.fullAccess.toggle() }
                }
            }
            .padding(14)

            Spacer()

            // Account footer
            HStack(spacing: 8) {
                Circle().fill(Color(hex: model.currentProfile.colorHex)).frame(width: 22, height: 22)
                    .overlay(Text(model.account.initials).font(.system(size: 9, weight: .semibold)).foregroundStyle(.white))
                VStack(alignment: .leading, spacing: 1) {
                    Text(model.account.name).font(.system(size: 11.5, weight: .medium)).foregroundStyle(Theme.textPrimary).lineLimit(1)
                    if !model.account.plan.isEmpty {
                        Text(model.account.plan).font(.system(size: 10)).foregroundStyle(Theme.textTertiary)
                    }
                }
                Spacer()
            }
            .padding(.horizontal, 14).padding(.vertical, 12)
            .overlay(alignment: .top) { Rectangle().fill(Theme.separator).frame(height: 1) }
        }
        .frame(width: 232)
        .background(Theme.canvas)
    }

    private func pickerRow(_ label: String, value: String, options: [String], onSelect: @escaping (String) -> Void) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(label).font(.system(size: 11)).foregroundStyle(Theme.textTertiary)
            Menu {
                ForEach(options, id: \.self) { opt in
                    Button(opt) { onSelect(opt) }
                }
            } label: {
                HStack(spacing: 6) {
                    Text(value).font(.system(size: 12)).foregroundStyle(Theme.textPrimary).lineLimit(1)
                    Spacer()
                    Image(systemName: "chevron.up.chevron.down").font(.system(size: 8)).foregroundStyle(Theme.textTertiary)
                }
                .padding(.horizontal, 10).frame(height: 30)
                .background(RoundedRectangle(cornerRadius: 7).fill(Theme.fieldFill)
                    .overlay(RoundedRectangle(cornerRadius: 7).strokeBorder(Theme.cardStroke, lineWidth: 1)))
            }
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            .fixedSize(horizontal: false, vertical: true)
        }
    }
}
