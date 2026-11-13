import SwiftUI
import UserNotifications

/// Local notifications for new Needs-an-answer rows, as the app's sync
/// sees them while it runs. A row notifies once, by key, and only if it
/// began after this phone started watching, so the first launch isn't a
/// flood. While a push lease is live the push notifies instead (`quiet`)
/// for the rows it covers, so nothing comes twice. Tapping one (or a push)
/// opens its row.
final class Notifier: NSObject, UNUserNotificationCenterDelegate {
    weak var model: Model?
    private var seen: [String]
    private var seenSet: Set<String>
    /// When this phone started watching (unix seconds).
    private let since: UInt64

    override init() {
        let d = UserDefaults.standard
        seen = d.stringArray(forKey: "needsSeen") ?? []
        seenSet = Set(seen)
        if let s = d.object(forKey: "needsSince") as? NSNumber {
            since = s.uint64Value
        } else {
            since = UInt64(Date().timeIntervalSince1970)
            d.set(NSNumber(value: since), forKey: "needsSince")
        }
        super.init()
    }

    func authorize() {
        let c = UNUserNotificationCenter.current()
        c.delegate = self
        c.requestAuthorization(options: [.alert, .sound]) { _, _ in }
    }

    /// On every sync poke (main thread). Only the watcher's rows notify,
    /// as with push. `quiet` (a push lease is live): rows with the
    /// watcher's ping out are the push's, so post nothing for those.
    func check(_ client: Client, quiet: Bool) {
        let fresh = client.needsYou().filter { $0.watcher && !seenSet.contains($0.key) }
        guard !fresh.isEmpty else { return }
        for r in fresh where r.at >= since && !(quiet && r.pushed) {
            post(r)
        }
        seen += fresh.map(\.key)
        if seen.count > 500 { seen.removeFirst(seen.count - 500) }
        seenSet = Set(seen)
        UserDefaults.standard.set(seen, forKey: "needsSeen")
    }

    private func post(_ r: NeedRow) {
        let content = UNMutableNotificationContent()
        content.title = "\(r.channelLabel) · \(r.whoName)"
        content.body = r.text
        content.sound = .default
        content.userInfo = ["link": r.link]
        UNUserNotificationCenter.current().add(
            UNNotificationRequest(identifier: r.key, content: content, trigger: nil))
    }

    /// Quiet while the app is open (lucian, 2026-10-21): no banner, no sound;
    /// it lands in Notification Center, and the tab's 🚩 and the bell show it.
    func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification,
                                withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void) {
        completionHandler([.list])
    }

    func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
                                withCompletionHandler completionHandler: @escaping () -> Void) {
        let link = response.notification.request.content.userInfo["link"] as? String
        DispatchQueue.main.async {
            if let link, let m = self.model, let c = m.client, let f = c.follow(url: link) {
                m.follow(f)
            }
            completionHandler()
        }
    }
}
