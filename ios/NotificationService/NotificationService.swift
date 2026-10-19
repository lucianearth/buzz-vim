import Foundation
import UserNotifications

/// Every push arrives as the gateway's content-free "Reconnect to your
/// relay now". This fetches the newest event the lease would have woken us
/// for and says it the way tab 0 does (`push_note`), falling back to a
/// plain line when that can't be found in time.
final class NotificationService: UNNotificationServiceExtension {
    private let lock = NSLock()
    private var handler: ((UNNotificationContent) -> Void)?
    private var content = UNMutableNotificationContent()

    override func didReceive(_ request: UNNotificationRequest,
                             withContentHandler contentHandler: @escaping (UNNotificationContent) -> Void) {
        handler = contentHandler
        content = (request.content.mutableCopy() as? UNMutableNotificationContent) ?? UNMutableNotificationContent()
        content.title = "Buzz Vim"
        content.body = "Something new needs you"
        content.sound = .default
        guard let secret = Keychain.read(), let relay = Keychain.read(account: "relay"),
              let subs = PushState.load().leased else {
            finish()
            return
        }
        Task {
            let note = await pushNote(relayUrl: relay, secret: secret, subscriptions: subs)
            let fresh: Bool = lock.withLock {
                // Too late: the fallback has gone out already.
                guard handler != nil else { return false }
                content.title = note.title
                content.body = note.body
                if let link = note.link { content.userInfo["link"] = link }
                return true
            }
            guard fresh else { return }
            finish()
        }
    }

    override func serviceExtensionTimeWillExpire() {
        finish()
    }

    /// Hand the content back once, whichever comes first.
    private func finish() {
        lock.lock()
        let h = handler
        handler = nil
        lock.unlock()
        h?(content)
    }
}
