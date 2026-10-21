import SwiftUI
import UIKit

struct ChannelView: View {
    let client: Client
    let channel: String
    let openThread: (String) -> Void
    @Environment(Model.self) private var model
    @Environment(\.theme) private var theme
    @State private var edges = Edges()
    @State private var fling = Fling()

    var body: some View {
        let _ = model.version
        let tl = client.timeline(channel: channel)
        ScrollViewReader { proxy in
            ScrollView {
                // Not lazy: a LazyVStack guesses the height of rows it hasn't
                // drawn, and bottom-anchored that guess showed as a screen of
                // empty space under the last message until the rows measured.
                VStack(alignment: .leading, spacing: 22) {
                    Color.clear.frame(height: 0).id("top")
                    if tl.hasMore {
                        ProgressView()
                            .frame(maxWidth: .infinity)
                            .onScrollVisibilityChange { seen in
                                if seen { client.loadOlder(channel: channel) }
                            }
                    }
                    if !tl.loaded {
                        Text("loading…")
                            .font(.small)
                            .foregroundStyle(theme.muted)
                    }
                    ForEach(tl.messages, id: \.id) { m in
                        MessageRow(client: client, message: m, showReplies: true, latest: m.id == tl.latest)
                            .equatable()
                            .contentShape(Rectangle())
                            .onTapGesture(count: 2) { client.thumbsUp(target: m.id) }
                            .onTapGesture { if !m.system { openThread(m.id) } }
                            .id(m.id)
                    }
                    if !tl.working.isEmpty {
                        Working(names: tl.working)
                    }
                    Color.clear.frame(height: 1).id("end")
                }
                .padding(.horizontal, 22)
                .padding(.top, 6)
                .background(FlingStop(fling: fling))
            }
            .defaultScrollAnchor(.bottom)
            .defaultScrollAnchor(.bottom, for: .sizeChanges)
            .scrollDismissesKeyboard(.interactively)
            .onScrollGeometryChange(for: Edges.self, of: Edges.init) { _, now in edges = now }
            .onChange(of: tl.messages.last?.id) { _, _ in
                withAnimation(.easeOut(duration: 0.2)) { proxy.scrollTo("end", anchor: .bottom) }
            }
            // Older messages came in above while you were up there: keep
            // the row that was on top, instead of landing on the oldest one
            // loaded. At the bottom (a page loading as the channel opens)
            // the bottom anchor keeps you where you were.
            .onChange(of: tl.messages.first?.id) { old, _ in
                guard !edges.bottom, let old, tl.messages.contains(where: { $0.id == old }) else { return }
                var t = Transaction()
                t.disablesAnimations = true
                withTransaction(t) { proxy.scrollTo(old, anchor: .top) }
            }
            // The bell key landed here: that message at the top.
            .onChange(of: model.scrollTarget?.id) { _, _ in scrollToTarget(proxy) }
            .onAppear { scrollToTarget(proxy) }
            .overlay(alignment: .bottomTrailing) {
                EdgeButtons(edges: edges) { top in
                    fling.stop()
                    withAnimation(.easeOut(duration: 0.25)) {
                        proxy.scrollTo(top ? "top" : "end", anchor: top ? .top : .bottom)
                    }
                }
            }
            .overlay(alignment: .bottomLeading) { BellButton() }
            .overlay(alignment: .bottom) { JumpChip() }
            .safeAreaInset(edge: .bottom) {
                ComposeBar(client: client, channel: channel, root: nil, hint: "write…")
            }
        }
    }

    private func scrollToTarget(_ proxy: ScrollViewProxy) {
        guard let t = model.scrollTarget, t.channel == channel else { return }
        model.scrollTarget = nil
        // After a tab swipe settles.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) {
            withAnimation(.easeOut(duration: 0.2)) { proxy.scrollTo(t.id, anchor: .top) }
        }
    }
}

/// Is the list at its top, at its bottom? Near enough counts.
struct Edges: Equatable {
    var top = true
    var bottom = true

    init() {}

    /// From the visible rect, in content coordinates: the inset math was
    /// off by the compose bar, so at the bottom it still read "away".
    init(_ g: ScrollGeometry) {
        top = g.visibleRect.minY <= 80
        bottom = g.visibleRect.maxY >= g.contentSize.height - 80
    }
}

/// Round ↑ and ↓ at the side of a list, each while you are away from that
/// end. The status-bar tap can't do it: there is a list per tab.
struct EdgeButtons: View {
    let edges: Edges
    let go: (_ top: Bool) -> Void
    @Environment(\.theme) private var theme

    var body: some View {
        VStack(spacing: 10) {
            if !edges.top { button("arrow.up") { go(true) } }
            if !edges.bottom { button("arrow.down") { go(false) } }
        }
        .padding(.trailing, 14)
        .padding(.bottom, 10)
        .animation(.easeOut(duration: 0.15), value: edges)
    }

    private func button(_ symbol: String, _ action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: symbol)
                .font(.system(size: 15, weight: .semibold))
                .foregroundStyle(theme.text)
                .frame(width: 38, height: 38)
                .background(theme.idle, in: Circle())
                .overlay(Circle().stroke(theme.rule, lineWidth: 1))
        }
        .transition(.opacity)
    }
}

/// The bell, round at the bottom left of a list: the newest activity in
/// this tab.
struct BellButton: View {
    @Environment(\.nav) private var nav
    @Environment(\.theme) private var theme

    var body: some View {
        Button { nav.bell() } label: {
            Image(systemName: "bell.badge")
                .font(.system(size: 15, weight: .semibold))
                .foregroundStyle(theme.text)
                .frame(width: 38, height: 38)
                .background(theme.idle, in: Circle())
                .overlay(Circle().stroke(theme.rule, lineWidth: 1))
        }
        .padding(.leading, 14)
        .padding(.bottom, 10)
    }
}

/// ↰ at the bottom centre while a jump (a link, the bell, a Needs you row)
/// can be undone: tap goes back one, hold lists the chain to pick from.
struct JumpChip: View {
    @Environment(\.nav) private var nav
    @Environment(\.theme) private var theme

    var body: some View {
        if !nav.jumps.isEmpty {
            Menu {
                // Top to bottom as written, oldest first, so the last
                // place sits nearest the chip.
                ForEach(Array(nav.jumps.enumerated()).reversed(), id: \.offset) { i, name in
                    Button(name) { nav.back(i + 1) }
                }
            } label: {
                Image(systemName: "arrow.uturn.backward")
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundStyle(theme.text)
                    .frame(width: 38, height: 38)
                    .background(theme.idle, in: Circle())
                    .overlay(Circle().stroke(theme.rule, lineWidth: 1))
            } primaryAction: {
                nav.back(1)
            }
            .menuOrder(.fixed)
            .padding(.bottom, 10)
            .transition(.opacity)
        }
    }
}

/// The list's UIScrollView, to stop a fling: while one runs, SwiftUI's
/// scrollTo is overridden by it and the top/bottom buttons go nowhere.
final class Fling {
    weak var view: UIScrollView?

    func stop() {
        guard let v = view, v.isDecelerating || v.isDragging else { return }
        v.setContentOffset(v.contentOffset, animated: false)
    }
}

/// Put inside a ScrollView's content: finds that UIScrollView for `fling`.
struct FlingStop: UIViewRepresentable {
    let fling: Fling

    func makeUIView(context: Context) -> UIView {
        let v = Finder()
        v.found = { [weak fling] in fling?.view = $0 }
        v.isUserInteractionEnabled = false
        return v
    }

    func updateUIView(_ v: UIView, context: Context) {}

    final class Finder: UIView {
        var found: ((UIScrollView) -> Void)?

        override func didMoveToWindow() {
            super.didMoveToWindow()
            var s = superview
            while let v = s, !(v is UIScrollView) { s = v.superview }
            if let sv = s as? UIScrollView { found?(sv) }
        }
    }
}

/// "taskbot thinking…" under the last message while an agent answers.
struct Working: View {
    let names: [String]
    @Environment(\.theme) private var theme

    var body: some View {
        HStack(spacing: 7) {
            Pulse(color: theme.live, on: true)
            Text(names.joined(separator: ", ") + " thinking…")
                .font(.small)
                .foregroundStyle(theme.muted)
        }
    }
}

/// Equatable so a redraw of the list (the 1 s tick while an agent works)
/// skips rows whose message hasn't changed.
struct MessageRow: View, Equatable {
    static func == (a: MessageRow, b: MessageRow) -> Bool {
        a.message == b.message && a.showReplies == b.showReplies && a.latest == b.latest
    }

    let client: Client
    let message: Message
    let showReplies: Bool
    /// The newest activity here: marked so it's easy to find.
    var latest = false
    @Environment(\.theme) private var theme
    @Environment(Model.self) private var model
    @State private var viewer: Viewer?

    var body: some View {
        let m = message
        if m.system {
            Text(plainSystem(m))
                .font(.small)
                .foregroundStyle(theme.faint)
        } else {
            VStack(alignment: .leading, spacing: 4) {
                HStack(alignment: .firstTextBaseline, spacing: 12) {
                    Text(m.authorName)
                        .font(.name)
                        .foregroundStyle(theme.ink(m.author))
                    Text(stamp(m.at))
                        .font(.stamp)
                        .foregroundStyle(theme.muted)
                    if m.edited {
                        Text("edited").font(.stamp).foregroundStyle(theme.faint)
                    }
                    if latest {
                        HStack(spacing: 4) {
                            Circle().fill(theme.accent).frame(width: 6, height: 6)
                            Text("latest").font(.stamp)
                        }
                        .foregroundStyle(theme.accent)
                    }
                }
                ForEach(Array(m.segments.enumerated()), id: \.offset) { _, s in
                    switch s {
                    case let .text(text):
                        Text(rich(text, mentions: m.mentionNames, theme: theme,
                                  label: { client.linkLabel(url: $0) },
                                  desktop: { client.opensInDesktop(url: $0) }))
                            .font(.text)
                            .lineSpacing(3)
                            .fixedSize(horizontal: false, vertical: true)
                    case let .code(lang, body):
                        CodeBlock(lang: lang, code: body)
                    }
                }
                let images = m.media.filter { $0.mime.hasPrefix("image/") || $0.mime.isEmpty }
                ForEach(Array(images.enumerated()), id: \.offset) { i, x in
                    RelayImage(client: client, url: x.url, width: x.width, height: x.height)
                        .frame(maxHeight: 260)
                        .clipShape(RoundedRectangle(cornerRadius: 12))
                        .onTapGesture { viewer = Viewer(urls: images.map(\.url), start: i) }
                }
                if m.state == .failed {
                    Text("not sent · tap to send again")
                        .font(.small)
                        .foregroundStyle(theme.badge)
                        .onTapGesture { client.retry(id: m.id) }
                        .contextMenu {
                            Button("Send again") { client.retry(id: m.id) }
                            Button("Discard, keep the text", role: .destructive) {
                                if let t = client.discard(id: m.id) {
                                    model.takeBack(id: m.id, text: t)
                                }
                            }
                        }
                }
                if !m.reactions.isEmpty {
                    HStack(spacing: 6) {
                        ForEach(m.reactions, id: \.emoji) { r in
                            Text("\(r.emoji) \(r.count)")
                                .font(.text)
                                .foregroundStyle(theme.text)
                                .padding(.horizontal, 11)
                                .padding(.vertical, 6)
                                .background(r.mine ? theme.idle : theme.card, in: RoundedRectangle(cornerRadius: 10))
                                .overlay(RoundedRectangle(cornerRadius: 10).stroke(theme.rule, lineWidth: 1))
                        }
                    }
                    .padding(.top, 4)
                }
                if showReplies && m.replyCount > 0 {
                    ReplyLine(client: client, message: m)
                        .padding(.top, 4)
                }
            }
            .opacity(m.state == .sending ? 0.55 : 1)
            .fullScreenCover(item: $viewer) { v in
                ImagePager(client: client, viewer: v)
            }
        }
    }

    private func plainSystem(_ m: Message) -> String {
        let t = m.segments.compactMap { s -> String? in
            if case let .text(text) = s { return text }
            return nil
        }.joined(separator: " ")
        return "\(m.authorName) " + t.trimmingCharacters(in: CharacterSet(charactersIn: "_"))
    }
}

/// Under a message with replies: who replied, how many, how many you
/// haven't seen, who wrote the last one (when we hold it) and how long ago. Tapping the message
/// opens the thread.
struct ReplyLine: View {
    let client: Client
    let message: Message
    @Environment(\.theme) private var theme

    var body: some View {
        let m = message
        HStack(spacing: 8) {
            HStack(spacing: -6) {
                ForEach(m.repliers.reversed(), id: \.pubkey) { p in
                    Avatar(client: client, person: p, size: 22)
                }
            }
            (Text("\(m.replyCount) \(m.replyCount == 1 ? "reply" : "replies")")
                .foregroundStyle(m.newReplies > 0 ? theme.text : theme.muted)
             + Text(m.newReplies > 0 ? " (\(m.newReplies) new)" : "")
                .foregroundStyle(theme.badge)
             + Text(m.lastReplyAt > 0 ? " · " + last(m) : "")
                .foregroundStyle(theme.faint))
                .font(.small)
                .lineLimit(1)
        }
    }

    private func last(_ m: Message) -> String {
        m.lastReplyBy.isEmpty ? ago(m.lastReplyAt) : "last \(m.lastReplyBy) " + ago(m.lastReplyAt)
    }
}

/// A round face: their picture, or their initial in their color.
struct Avatar: View {
    let client: Client
    let person: Person
    let size: CGFloat
    @Environment(\.theme) private var theme

    var body: some View {
        Group {
            if person.picture.isEmpty {
                Text(person.name.prefix(1).uppercased())
                    .font(.system(size: size * 0.5, weight: .semibold))
                    .foregroundStyle(theme.onAccent)
                    .frame(width: size, height: size)
                    .background(theme.ink(person.name))
            } else {
                RelayImage(client: client, url: person.picture, width: 1, height: 1, fill: true,
                           maxPixels: Int(size * 3))
                    .frame(width: size, height: size)
            }
        }
        .clipShape(Circle())
        .overlay(Circle().stroke(theme.canvas, lineWidth: 2))
    }
}

/// "just now", "5m ago", "3h ago", "2d ago".
func ago(_ at: UInt64) -> String {
    let s = max(0, Int(Date().timeIntervalSince1970) - Int(at))
    switch s {
    case ..<60: return "just now"
    case ..<3600: return "\(s / 60)m ago"
    case ..<86_400: return "\(s / 3600)h ago"
    default: return "\(s / 86_400)d ago"
    }
}

struct CodeBlock: View {
    let lang: String
    let code: String
    @Environment(\.theme) private var theme

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            if !lang.isEmpty {
                Text(lang).font(.tiny).foregroundStyle(theme.codeLabel)
                    .frame(maxWidth: .infinity, alignment: .trailing)
            }
            ScrollView(.horizontal, showsIndicators: false) {
                Text(code)
                    .font(.mono)
                    .foregroundStyle(theme.code)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: true, vertical: false)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(theme.codeBg, in: RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).stroke(theme.rule, lineWidth: 1))
    }
}

private func formatter(_ setup: (DateFormatter) -> Void) -> DateFormatter {
    let f = DateFormatter()
    setup(f)
    return f
}

private let todayStamp = formatter { $0.timeStyle = .short }
private let weekStamp = formatter { $0.setLocalizedDateFormatFromTemplate("EEE j:mm") }
private let olderStamp = formatter { $0.setLocalizedDateFormatFromTemplate("MMM d") }

/// "9:41" today, "Mon 9:41" this week, "Oct 3" before that.
func stamp(_ at: UInt64) -> String {
    let d = Date(timeIntervalSince1970: TimeInterval(at))
    let cal = Calendar.current
    if cal.isDateInToday(d) {
        return todayStamp.string(from: d)
    } else if let days = cal.dateComponents([.day], from: d, to: Date()).day, days < 6 {
        return weekStamp.string(from: d)
    }
    return olderStamp.string(from: d)
}

private let detector = try? NSDataDetector(types: NSTextCheckingResult.CheckingType.link.rawValue)

/// Inline markdown, with bare URLs made tappable and @mentions in the accent.
/// `label` names a bare buzz:// link by where it goes, as the TUI does;
/// links `desktop` says Buzz Desktop opens get a 🐝, as Desktop shows them.
func rich(_ src: String, mentions: [String], theme: Theme,
          label: (String) -> String? = { _ in nil },
          desktop: (String) -> Bool = { _ in false }) -> AttributedString {
    let opts = AttributedString.MarkdownParsingOptions(interpretedSyntax: .inlineOnlyPreservingWhitespace)
    var a = (try? AttributedString(markdown: src, options: opts)) ?? AttributedString(src)
    a.foregroundColor = theme.text
    let plain = String(a.characters)

    func span(_ r: Range<String.Index>) -> Range<AttributedString.Index>? {
        let lo = plain.distance(from: plain.startIndex, to: r.lowerBound)
        let n = plain.distance(from: r.lowerBound, to: r.upperBound)
        guard lo + n <= a.characters.count else { return nil }
        let start = a.characters.index(a.startIndex, offsetBy: lo)
        let end = a.characters.index(start, offsetBy: n)
        return start..<end
    }

    // buzz:// links first: the data detector doesn't know the scheme.
    for m in plain.matches(of: #/buzz:\/\/[^\s<>()\[\]"']+/#) {
        var r = m.range
        while r.upperBound > r.lowerBound, ".,;:!?".contains(plain[plain.index(before: r.upperBound)]) {
            r = r.lowerBound..<plain.index(before: r.upperBound)
        }
        guard let url = URL(string: String(plain[r])), let s = span(r), a[s].link == nil else { continue }
        a[s].link = url
    }
    // Only links written with their scheme: the detector also takes file
    // names like lib.rs (.rs is a country) for web addresses.
    if let det = detector {
        let ns = NSRange(plain.startIndex..., in: plain)
        for match in det.matches(in: plain, range: ns) {
            guard let r = Range(match.range, in: plain), let url = match.url,
                  plain[r].hasPrefix("http://") || plain[r].hasPrefix("https://"),
                  let s = span(r), a[s].link == nil else { continue }
            a[s].link = url
        }
    }
    for run in a.runs where run.link != nil {
        a[run.range].foregroundColor = theme.link
        a[run.range].underlineStyle = Text.LineStyle.single
    }
    for name in Set(mentions) {
        var from = plain.startIndex
        while let r = plain.range(of: "@" + name, range: from..<plain.endIndex) {
            if let s = span(r) {
                a[s].foregroundColor = theme.accent
                a[s].font = Font.textBold
            }
            from = r.upperBound
        }
    }
    // Last, so the ranges above still match `plain`.
    var out = AttributedString()
    var prev: URL?
    for run in a.runs {
        let piece = AttributedString(a[run.range])
        if let url = run.link, url != prev, url.scheme == "buzz", desktop(url.absoluteString) {
            var bee = AttributedString("🐝 ", attributes: run.attributes)
            bee.underlineStyle = nil
            out.append(bee)
        }
        prev = run.link
        if let url = run.link, url.scheme == "buzz",
           String(piece.characters) == url.absoluteString,
           let l = label(url.absoluteString) {
            out.append(AttributedString(l, attributes: run.attributes))
        } else {
            out.append(piece)
        }
    }
    return out
}
