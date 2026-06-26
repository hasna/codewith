import SwiftUI

/// "Get started with CodeWith" — login / register for all supported providers.
struct LoginView: View {
    @Bindable var model: AppModel
    @State private var mode: Mode = .home
    @State private var apiKey = ""
    @State private var selectedProvider = "OpenAI"
    @Environment(\.snapshotMode) private var snapshot

    enum Mode { case home, providers, apiKey }

    private struct Provider: Identifiable {
        var id: String { name }
        var name: String
        var icon: String
        var keyBased: Bool
    }
    private let providers: [Provider] = [
        .init(name: "OpenAI", icon: "key.fill", keyBased: true),
        .init(name: "Anthropic", icon: "slider.horizontal.3", keyBased: false),
        .init(name: "Azure", icon: "slider.horizontal.3", keyBased: false),
        .init(name: "OpenRouter", icon: "slider.horizontal.3", keyBased: false),
        .init(name: "Ollama", icon: "desktopcomputer", keyBased: false),
    ]

    var body: some View {
        ZStack {
            Theme.canvas
            VStack {
                HStack {
                    RoundedRectangle(cornerRadius: 8, style: .continuous)
                        .fill(Theme.accent)
                        .frame(width: 44, height: 26)
                        .overlay(Image(systemName: "person.crop.rectangle").font(.system(size: 12)).foregroundStyle(.white))
                    Spacer()
                }
                Spacer()
            }
            .padding(18)

            VStack(spacing: 0) {
                BrandBlob().frame(width: 88, height: 88).padding(.bottom, 30)

                switch mode {
                case .home:      homeContent
                case .providers: providerList
                case .apiKey:    apiKeyEntry
                }

                if let err = model.loginError {
                    Text(err).font(.system(size: 12)).foregroundStyle(Theme.danger).padding(.top, 14)
                }
            }
            .frame(width: 380)
        }
    }

    // MARK: Home (ChatGPT + another way)

    private var homeContent: some View {
        VStack(spacing: 0) {
            Text("Get started with CodeWith")
                .font(.system(size: 30, weight: .medium)).foregroundStyle(Theme.textPrimary)
                .padding(.bottom, 30)

            primaryButton(icon: "", title: model.loginInProgress ? "Waiting for browser…" : "Sign in with ChatGPT") {
                Task { await model.loginWithChatGPT() }
            }
            .disabled(model.loginInProgress)
            .padding(.bottom, 12)

            secondaryButton(title: "Sign in another way") { mode = .providers }
                .disabled(model.loginInProgress)
                .padding(.bottom, 22)

            Button { openURL("https://chatgpt.com") } label: {
                Text("Sign up").font(.system(size: 14)).foregroundStyle(Theme.textSecondary).underline()
            }.buttonStyle(.plain)
        }
    }

    // MARK: Provider list

    private var providerList: some View {
        VStack(spacing: 0) {
            Text("Choose a provider")
                .font(.system(size: 22, weight: .medium)).foregroundStyle(Theme.textPrimary)
                .padding(.bottom, 22)
            VStack(spacing: 8) {
                ForEach(providers) { p in
                    Button {
                        selectedProvider = p.name
                        if p.keyBased { mode = .apiKey } else { Task { await model.loginWithoutApiKey(providerName: p.name) } }
                    } label: {
                        HStack(spacing: 12) {
                            Image(systemName: p.icon).font(.system(size: 14)).foregroundStyle(Theme.textSecondary).frame(width: 20)
                            Text(p.name).font(.system(size: 14, weight: .medium)).foregroundStyle(Theme.textPrimary)
                            Spacer()
                            Image(systemName: "chevron.right").font(.system(size: 11)).foregroundStyle(Theme.textTertiary)
                        }
                        .padding(.horizontal, 14).frame(height: 48).contentShape(Rectangle())
                        .background(RoundedRectangle(cornerRadius: 12).fill(Theme.fieldFill)
                            .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Theme.cardStroke, lineWidth: 1)))
                    }.buttonStyle(.plain).disabled(model.loginInProgress)
                }
            }
            backLink { mode = .home }
        }
    }

    // MARK: API key entry

    private var apiKeyEntry: some View {
        VStack(spacing: 0) {
            Text("Sign in with \(selectedProvider)")
                .font(.system(size: 22, weight: .medium)).foregroundStyle(Theme.textPrimary)
                .padding(.bottom, 8)
            Text("Paste your \(selectedProvider) API key.")
                .font(.system(size: 12)).foregroundStyle(Theme.textSecondary).padding(.bottom, 18)

            HStack {
                if snapshot {
                    Text(apiKey.isEmpty ? "sk-…" : "••••••••").font(.system(size: 13)).foregroundStyle(Theme.textTertiary)
                } else {
                    SecureField("sk-…", text: $apiKey).textFieldStyle(.plain).font(.system(size: 13))
                }
                Spacer()
            }
            .padding(.horizontal, 14).frame(height: 46)
            .background(RoundedRectangle(cornerRadius: 12).fill(Theme.fieldFill)
                .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Theme.cardStroke, lineWidth: 1)))
            .padding(.bottom, 12)

            primaryButton(icon: "checkmark", title: model.loginInProgress ? "Signing in…" : "Continue") {
                Task { await model.loginWithApiKey(apiKey, providerName: selectedProvider) }
            }
            .disabled(model.loginInProgress)
            backLink { mode = .providers }
        }
    }

    // MARK: Components

    private func primaryButton(icon: String, title: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 10) {
                if !icon.isEmpty {
                    Image(systemName: icon).font(.system(size: 13, weight: .semibold)).foregroundStyle(.white)
                }
                Text(title).font(.system(size: 15, weight: .semibold)).foregroundStyle(.white)
            }
            .frame(width: 360, height: 52).contentShape(Rectangle())
            .background(Capsule().fill(Theme.accent))
        }.buttonStyle(.plain)
    }
    private func secondaryButton(title: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title).font(.system(size: 15, weight: .semibold)).foregroundStyle(Theme.textPrimary)
                .frame(width: 360, height: 52).contentShape(Rectangle())
                .background(Capsule().fill(Color.white).overlay(Capsule().strokeBorder(Theme.cardStroke, lineWidth: 1)))
        }.buttonStyle(.plain)
    }
    private func backLink(_ action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text("Back").font(.system(size: 13)).foregroundStyle(Theme.textSecondary)
        }.buttonStyle(.plain).padding(.top, 18)
    }
    private func openURL(_ s: String) {
        #if canImport(AppKit)
        if let url = URL(string: s) { NSWorkspace.shared.open(url) }
        #endif
    }
}

/// A soft multi-lobed "cloud/flower" blob mark with a thin `>_` prompt glyph.
struct BrandBlob: View {
    private let grad = LinearGradient(colors: [Theme.accent, Color(hex: 0x262626)],
                                      startPoint: .topLeading, endPoint: .bottomTrailing)
    var body: some View {
        ZStack {
            // 8 outer lobes + center → a fuller cloud closer to the reference mark.
            ForEach(Array([CGPoint(x: 0, y: -0.52), CGPoint(x: 0.37, y: -0.37), CGPoint(x: 0.52, y: 0),
                           CGPoint(x: 0.37, y: 0.37), CGPoint(x: 0, y: 0.52), CGPoint(x: -0.37, y: 0.37),
                           CGPoint(x: -0.52, y: 0), CGPoint(x: -0.37, y: -0.37), CGPoint(x: 0, y: 0)].enumerated()), id: \.offset) { _, p in
                Circle().frame(width: 38, height: 38).offset(x: p.x * 32, y: p.y * 32)
            }
        }
        .foregroundStyle(grad)
        .compositingGroup()
        .overlay {
            HStack(spacing: 3) {
                Image(systemName: "chevron.right").font(.system(size: 16, weight: .semibold))
                Rectangle().frame(width: 9, height: 2.5).cornerRadius(1).offset(y: 6)
            }
            .foregroundStyle(.white)
        }
        .shadow(color: Color.black.opacity(0.08), radius: 5, y: 2)
    }
}
