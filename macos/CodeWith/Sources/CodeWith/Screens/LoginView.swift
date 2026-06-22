import SwiftUI

/// "Get started with CodeWith" (reference screenshot 11).
struct LoginView: View {
    var body: some View {
        ZStack {
            Theme.canvas
            // Top-left brand pill
            VStack {
                HStack {
                    RoundedRectangle(cornerRadius: 8, style: .continuous)
                        .fill(LinearGradient(colors: [Color(hex: 0x6E6BF2), Color(hex: 0x4B47E0)], startPoint: .top, endPoint: .bottom))
                        .frame(width: 44, height: 26)
                        .overlay(Image(systemName: "person.crop.rectangle").font(.system(size: 12)).foregroundStyle(.white))
                    Spacer()
                }
                Spacer()
            }
            .padding(18)

            VStack(spacing: 0) {
                // Brand glyph — soft organic blob with a thin terminal prompt.
                BrandBlob()
                    .frame(width: 72, height: 72)
                    .padding(.bottom, 30)

                Text("Get started with CodeWith")
                    .font(.system(size: 29, weight: .medium)).foregroundStyle(Theme.textPrimary)
                    .padding(.bottom, 38)

                // Primary sign-in
                HStack(spacing: 10) {
                    Image(systemName: "chevron.left.forwardslash.chevron.right").font(.system(size: 13, weight: .bold)).foregroundStyle(.white)
                    Text("Sign in with CodeWith").font(.system(size: 15, weight: .semibold)).foregroundStyle(.white)
                }
                .frame(width: 360, height: 54)
                .background(Capsule().fill(Color(hex: 0x1A1A1A)))
                .padding(.bottom, 12)

                Text("Sign in another way")
                    .font(.system(size: 15, weight: .semibold)).foregroundStyle(Theme.textPrimary)
                    .frame(width: 360, height: 54)
                    .background(Capsule().fill(Color.white).overlay(Capsule().strokeBorder(Theme.cardStroke, lineWidth: 1)))
                    .padding(.bottom, 22)

                Text("Sign up").font(.system(size: 14)).foregroundStyle(Theme.textSecondary).underline()
            }
        }
    }
}

/// A soft multi-lobed "cloud/flower" blob mark with a thin `>_` prompt glyph.
struct BrandBlob: View {
    private let grad = LinearGradient(colors: [Color(hex: 0xB5AEF7), Color(hex: 0x8A86F0), Color(hex: 0x5B6CF0)],
                                      startPoint: .topLeading, endPoint: .bottomTrailing)
    var body: some View {
        ZStack {
            // Lobes
            ForEach(Array([CGPoint(x: 0, y: -0.5), CGPoint(x: 0.5, y: -0.1), CGPoint(x: 0.42, y: 0.42),
                           CGPoint(x: -0.42, y: 0.42), CGPoint(x: -0.5, y: -0.1), CGPoint(x: 0, y: 0.2)].enumerated()), id: \.offset) { _, p in
                Circle().frame(width: 40, height: 40).offset(x: p.x * 30, y: p.y * 30)
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
        .shadow(color: Color(hex: 0x4B47E0).opacity(0.14), radius: 5, y: 2)
    }
}
