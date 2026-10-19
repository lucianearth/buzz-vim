import SwiftUI
import UserNotifications

/// Tab 0, Needs you: pinned first, never in the switcher, so it can't be
/// moved or archived.
enum Needs {
    static let id = "needs-you"
}

/// Tab 0's page: what needs an answer (oldest first), then FYI (newest first). Tap a row
/// to open its message; Reply answers it in its thread from here.
struct NeedsView: View {
    let client: Client
    let open: (String) -> Void
    @Environment(Model.self) private var model
    @Environment(\.theme) private var theme
    /// The row with its reply box open, by key.
    @State private var replying: String?
    @State private var text = ""
    @FocusState private var focused: Bool

    var body: some View {
        let _ = model.version
        let rows = client.needsYou()
        let answer = rows.filter { $0.section == .answer }
        let fyi = rows.filter { $0.section == .fyi }
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 0) {
                if let failure = Push.shared.failure {
                    Text("push off: \(failure)")
                        .font(.small)
                        .foregroundStyle(theme.muted)
                        .textSelection(.enabled)
                        .padding(.top, 12)
                }
                if rows.isEmpty {
                    Text("nothing needs you")
                        .font(.small)
                        .foregroundStyle(theme.muted)
                        .frame(maxWidth: .infinity)
                        .padding(.top, 60)
                }
                if !answer.isEmpty {
                    heading("needs an answer", answer.count)
                    ForEach(answer, id: \.key) { row($0) }
                }
                if !fyi.isEmpty {
                    heading("FYI", fyi.count)
                    ForEach(fyi, id: \.key) { row($0) }
                }
            }
            .padding(.horizontal, 22)
            .padding(.bottom, 40)
        }
        .scrollDismissesKeyboard(.interactively)
    }

    private func heading(_ name: String, _ n: Int) -> some View {
        Text("\(name) (\(n))")
            .font(.label)
            .foregroundStyle(theme.bright)
            .padding(.top, 18)
            .padding(.bottom, 4)
    }

    private func row(_ r: NeedRow) -> some View {
        let answer = r.section == .answer
        return VStack(alignment: .leading, spacing: 6) {
            // Tap here to open it; the reply box below keeps its own taps.
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 8) {
                    Text(r.channelLabel).foregroundStyle(theme.muted)
                    Text(r.whoName).foregroundStyle(theme.text)
                    Text(r.kind).foregroundStyle(theme.faint)
                    Spacer()
                    Text(ago(r.at)).foregroundStyle(theme.faint)
                }
                .font(.small)
                .lineLimit(1)
                Text(r.text)
                    .font(.text)
                    .foregroundStyle(answer ? theme.bright : theme.muted)
                    .lineLimit(2)
            }
            .contentShape(Rectangle())
            .onTapGesture { open(r.link) }
            if replying == r.key {
                HStack(alignment: .bottom, spacing: 10) {
                    TextField("reply in its thread", text: $text, axis: .vertical)
                        .font(.text)
                        .lineLimit(1...5)
                        .focused($focused)
                    Button("Send") { send(r) }
                        .font(.button)
                        .foregroundStyle(theme.accent)
                }
                .padding(10)
                .background(theme.card, in: RoundedRectangle(cornerRadius: 10))
            } else {
                Button {
                    // Text typed for another row doesn't follow to this one.
                    text = ""
                    replying = r.key
                    focused = true
                } label: {
                    Label("Reply", systemImage: "arrowshape.turn.up.left")
                        .font(.small)
                        .foregroundStyle(theme.accent)
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.vertical, 12)
        .overlay(alignment: .bottom) {
            Rectangle().fill(theme.rule).frame(height: 1)
        }
    }

    /// Into the row's thread, notifying whoever asked.
    private func send(_ r: NeedRow) {
        let body = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !body.isEmpty else { return }
        let key = r.thread.map { "\(r.channel)/\($0)" } ?? r.channel
        do {
            let id = try client.send(channel: r.channel, root: r.thread, text: body,
                                     attachments: [], to: r.who.isEmpty ? [] : [r.who])
            model.sent(id: id, key: key, text: body)
            text = ""
            replying = nil
            focused = false
            model.say("replied in \(r.channelLabel)")
        } catch let BuzzError.Failed(message) {
            model.say("not sent: \(message)")
        } catch {
            model.say("not sent: \(error.localizedDescription)")
        }
    }

    private func ago(_ at: UInt64) -> String {
        let now = UInt64(Date().timeIntervalSince1970)
        let s = at > now ? 0 : now - at
        if s < 60 { return "\(s)s" }
        if s < 3600 { return "\(s / 60)m" }
        if s < 86_400 { return "\(s / 3600)h" }
        return "\(s / 86_400)d"
    }
}

/// Local notifications for new Needs-an-answer rows, as the app's sync
/// sees them while it runs. A row notifies once, by key, and only if it
/// began after this phone started watching, so the first launch isn't a
/// flood. While a push lease is live the push notifies instead (`quiet`),
/// so nothing comes twice. Tapping one (or a push) opens its row.
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
    /// as with push. `quiet` (a push lease is live and covers them): mark
    /// rows seen, post nothing.
    func check(_ client: Client, quiet: Bool) {
        let fresh = client.needsYou().filter { $0.watcher && !seenSet.contains($0.key) }
        guard !fresh.isEmpty else { return }
        for r in fresh where r.at >= since && !quiet {
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

    /// Show them while the app is open too.
    func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification,
                                withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void) {
        completionHandler([.banner, .sound])
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
