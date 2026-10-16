import SwiftUI
import UIKit

struct ChannelView: View {
    let client: Client
    let channel: String
    let openThread: (String) -> Void
    @Environment(Model.self) private var model
    @Environment(\.theme) private var theme

    var body: some View {
        let _ = model.version
        let tl = client.timeline(channel: channel)
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 22) {
                    if tl.hasMore {
                        ProgressView()
                            .frame(maxWidth: .infinity)
                            .onAppear { client.loadOlder(channel: channel) }
                    }
                    if !tl.loaded {
                        Text("loading…")
                            .font(.small)
                            .foregroundStyle(theme.muted)
                    }
                    ForEach(tl.messages, id: \.id) { m in
                        MessageRow(client: client, message: m, showReplies: true, latest: m.id == tl.latest)
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
            }
            .defaultScrollAnchor(.bottom)
            .scrollDismissesKeyboard(.interactively)
            .onChange(of: tl.messages.last?.id) { _, _ in
                withAnimation(.easeOut(duration: 0.2)) { proxy.scrollTo("end", anchor: .bottom) }
            }
            // The bell key landed here: that message at the top.
            .onChange(of: model.scrollTarget?.id) { _, _ in scrollToTarget(proxy) }
            .onAppear { scrollToTarget(proxy) }
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

struct MessageRow: View {
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
                        Text(rich(text, mentions: m.mentionNames, theme: theme))
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
/// haven't seen, and how long ago the last one came. Tapping the message
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
             + Text(m.lastReplyAt > 0 ? " · " + ago(m.lastReplyAt) : "")
                .foregroundStyle(theme.faint))
                .font(.small)
                .lineLimit(1)
        }
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
                RelayImage(client: client, url: person.picture, width: 1, height: 1, fill: true)
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

/// "9:41" today, "Mon 9:41" this week, "Oct 3" before that.
func stamp(_ at: UInt64) -> String {
    let d = Date(timeIntervalSince1970: TimeInterval(at))
    let cal = Calendar.current
    let f = DateFormatter()
    if cal.isDateInToday(d) {
        f.timeStyle = .short
    } else if let days = cal.dateComponents([.day], from: d, to: Date()).day, days < 6 {
        f.setLocalizedDateFormatFromTemplate("EEE j:mm")
    } else {
        f.setLocalizedDateFormatFromTemplate("MMM d")
    }
    return f.string(from: d)
}

private let detector = try? NSDataDetector(types: NSTextCheckingResult.CheckingType.link.rawValue)

/// Inline markdown, with bare URLs made tappable and @mentions in the accent.
func rich(_ src: String, mentions: [String], theme: Theme) -> AttributedString {
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

    if let det = detector {
        let ns = NSRange(plain.startIndex..., in: plain)
        for match in det.matches(in: plain, range: ns) {
            guard let r = Range(match.range, in: plain), let url = match.url,
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
    return a
}
