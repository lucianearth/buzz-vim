import SwiftUI
import UIKit

@main
struct BuzzVimApp: App {
    @UIApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @State private var model = Model()
    @Environment(\.scenePhase) private var phase

    var body: some Scene {
        WindowGroup {
            Root()
                .environment(model)
                .modifier(Themed())
        }
        .onChange(of: phase) { _, now in
            switch now {
            case .background:
                model.client?.setVisible(visible: false)
                model.client?.flush()
                model.saveContext()
            case .active:
                Push.shared.retryStop()
                model.client?.setVisible(visible: true)
                model.client?.resume()
            default: break
            }
        }
    }
}

struct Root: View {
    @Environment(Model.self) private var model
    @Environment(\.theme) private var theme

    var body: some View {
        ZStack(alignment: .top) {
            theme.canvas.ignoresSafeArea()
            if let client = model.client {
                Home(client: client)
                    .environment(\.openURL, OpenURLAction { model.open($0, client: client) })
            } else {
                LoginView()
            }
            if let n = model.notice {
                Text(n)
                    .font(.small)
                    .foregroundStyle(theme.text)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 8)
                    .background(theme.idle, in: Capsule())
                    .padding(.top, 4)
                    .transition(.move(edge: .top).combined(with: .opacity))
                    .onTapGesture { model.notice = nil }
            }
        }
        .animation(.easeOut(duration: 0.2), value: model.notice)
    }
}

/// Swipe from the left edge goes back even though we draw our own header
/// and hide the navigation bar.
extension UINavigationController: UIGestureRecognizerDelegate {
    override open func viewDidLoad() {
        super.viewDidLoad()
        interactivePopGestureRecognizer?.delegate = self
    }

    public func gestureRecognizerShouldBegin(_ g: UIGestureRecognizer) -> Bool {
        viewControllers.count > 1
    }
}
