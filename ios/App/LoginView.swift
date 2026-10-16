import SwiftUI

struct LoginView: View {
    @Environment(Model.self) private var model
    @Environment(\.theme) private var theme
    @State private var relay = Model.defaultRelay
    @State private var secret = ""
    @State private var pairing = false
    @State private var withKey = false

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            Spacer()
            Text("buzz")
                .font(.system(size: 40, weight: .bold, design: .monospaced))
                .foregroundStyle(theme.accent)
            Button { pairing = true } label: {
                Text("Pair with Desktop")
                    .font(.body.weight(.semibold))
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 14)
                    .foregroundStyle(theme.onAccent)
                    .background(theme.accent, in: RoundedRectangle(cornerRadius: 14))
            }
            Text("Desktop shows a code; scan it, check the six digits match, done.")
                .font(.footnote)
                .foregroundStyle(theme.muted)
            if withKey {
                Text("Your private key is in Buzz Desktop under Settings → Profile → Private key. It stays in this phone's Keychain.")
                    .font(.footnote)
                    .foregroundStyle(theme.muted)
                field("relay", text: $relay, secure: false)
                field("nsec1…", text: $secret, secure: true)
                Button {
                    model.login(relay: relay, secret: secret)
                } label: {
                    Text("Log in")
                        .font(.body)
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 14)
                        .foregroundStyle(theme.text)
                        .background(theme.idle, in: RoundedRectangle(cornerRadius: 14))
                }
                .disabled(secret.isEmpty)
                .opacity(secret.isEmpty ? 0.5 : 1)
            } else {
                Button("Use a private key instead") { withKey = true }
                    .font(.footnote)
                    .foregroundStyle(theme.muted)
            }
            if let e = model.loginError {
                Text(e).font(.footnote).foregroundStyle(theme.badge)
            }
            Spacer()
            Spacer()
        }
        .padding(28)
        .sheet(isPresented: $pairing) {
            PairView()
                .presentationDragIndicator(.visible)
                .modifier(Themed())
        }
    }

    @ViewBuilder
    private func field(_ hint: String, text: Binding<String>, secure: Bool) -> some View {
        Group {
            if secure {
                SecureField(hint, text: text)
            } else {
                TextField(hint, text: text)
                    .keyboardType(.URL)
            }
        }
        .textInputAutocapitalization(.never)
        .autocorrectionDisabled()
        .font(.mono)
        .foregroundStyle(theme.text)
        .padding(14)
        .background(theme.card, in: RoundedRectangle(cornerRadius: 12))
    }
}
