import SwiftUI
import UIKit
import VisionKit

/// Log in by pairing with Buzz Desktop: scan its QR (or paste the
/// nostrpair:// link), compare the six digits, say yes on both screens.
struct PairView: View {
    @Environment(Model.self) private var model
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    @State private var flow = PairFlow()
    @State private var link = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            Text("Pair with Desktop")
                .font(.heading)
                .foregroundStyle(theme.bright)
            switch flow.step {
            case nil:
                Text("In Buzz Desktop: Settings → Mobile → Pair your mobile app. Point the camera at the code.")
                    .font(.small)
                    .foregroundStyle(theme.muted)
                if DataScannerViewController.isSupported {
                    QRScanner { flow.start($0, model: model) }
                        .frame(height: 300)
                        .clipShape(RoundedRectangle(cornerRadius: 16))
                }
                HStack(spacing: 10) {
                    TextField("or paste the nostrpair:// link", text: $link)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .font(.mono)
                        .foregroundStyle(theme.text)
                    Button("Go") { flow.start(link, model: model) }
                        .disabled(link.isEmpty)
                        .foregroundStyle(theme.accent)
                }
                .padding(14)
                .background(theme.card, in: RoundedRectangle(cornerRadius: 12))
            case .connecting:
                waiting("connecting to Desktop…")
            case let .compare(code, desktopSaidYes):
                Text("Does Desktop show this code?")
                    .font(.text)
                    .foregroundStyle(theme.text)
                Text(spaced(code))
                    .font(.custom("FiraMono-Medium", size: 44))
                    .foregroundStyle(theme.accent)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 12)
                if desktopSaidYes {
                    big("Yes, it matches", filled: true) { flow.pairing?.answer(same: true) }
                } else {
                    Text("Confirm on Desktop first, then here.")
                        .font(.small)
                        .foregroundStyle(theme.muted)
                }
                big("No", filled: false) {
                    flow.pairing?.answer(same: false)
                }
            case .receiving:
                waiting("receiving your identity…")
            case let .done(relay, secret):
                waiting("logging in…")
                    .onAppear {
                        model.login(relay: relay.isEmpty ? Model.defaultRelay : relay, secret: secret)
                        dismiss()
                    }
            case let .failed(message):
                Text(message)
                    .font(.text)
                    .foregroundStyle(theme.badge)
                big("Try again", filled: true) { flow.reset() }
            }
            Spacer()
        }
        .padding(24)
        .background(theme.sheet)
        .onDisappear { flow.pairing?.cancel() }
    }

    private func waiting(_ s: String) -> some View {
        HStack(spacing: 10) {
            ProgressView()
            Text(s).font(.small).foregroundStyle(theme.muted)
        }
    }

    private func big(_ title: String, filled: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title)
                .font(filled ? .button : .label)
                .frame(maxWidth: .infinity)
                .padding(.vertical, 14)
                .foregroundStyle(filled ? theme.onAccent : theme.text)
                .background(filled ? theme.accent : theme.idle, in: RoundedRectangle(cornerRadius: 14))
        }
    }

    private func spaced(_ code: String) -> String {
        code.count == 6 ? "\(code.prefix(3)) \(code.suffix(3))" : code
    }
}

@Observable
final class PairFlow {
    private(set) var pairing: Pairing?
    private(set) var step: PairStep?

    func start(_ qr: String, model: Model) {
        guard pairing == nil else { return }
        do {
            let p = try Pairing(qr: qr, listener: PairPoke(flow: self))
            pairing = p
            step = p.step()
        } catch let BuzzError.Failed(message) {
            step = .failed(message: message)
        } catch {
            step = .failed(message: error.localizedDescription)
        }
    }

    func refresh() {
        DispatchQueue.main.async {
            if let p = self.pairing { self.step = p.step() }
        }
    }

    func reset() {
        pairing?.cancel()
        pairing = nil
        step = nil
    }
}

final class PairPoke: Listener, @unchecked Sendable {
    weak var flow: PairFlow?
    init(flow: PairFlow) { self.flow = flow }
    func changed() { flow?.refresh() }
}

/// The camera, looking for one QR code.
struct QRScanner: UIViewControllerRepresentable {
    let found: (String) -> Void

    func makeUIViewController(context: Context) -> DataScannerViewController {
        let vc = DataScannerViewController(
            recognizedDataTypes: [.barcode(symbologies: [.qr])],
            qualityLevel: .balanced,
            isHighlightingEnabled: true
        )
        vc.delegate = context.coordinator
        try? vc.startScanning()
        return vc
    }

    func updateUIViewController(_ vc: DataScannerViewController, context: Context) {}

    func makeCoordinator() -> Coordinator { Coordinator(found: found) }

    final class Coordinator: NSObject, DataScannerViewControllerDelegate {
        let found: (String) -> Void
        private var done = false
        init(found: @escaping (String) -> Void) { self.found = found }

        func dataScanner(_ scanner: DataScannerViewController, didAdd items: [RecognizedItem],
                         allItems: [RecognizedItem]) {
            for item in items {
                if case let .barcode(code) = item, let s = code.payloadStringValue,
                   s.hasPrefix("nostrpair://"), !done {
                    done = true
                    scanner.stopScanning()
                    found(s)
                }
            }
        }
    }
}
