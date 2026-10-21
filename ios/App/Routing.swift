import SwiftUI

/// The Routing page: pinned last, after the channels, never in the
/// switcher (like tab 0).
enum Routing {
    static let id = "routing"
}

/// The page's channel, or nil for tab 0 and Routing, which aren't channels.
func channelPage(_ id: String?) -> String? {
    guard let id, id != Needs.id, id != Routing.id else { return nil }
    return id
}

/// How the watcher routed every stop it judged (`buzz_vim_core::routing`),
/// as the pipeline lucian approved: one trunk of stops, a coloured strip
/// per exit stream (width by count) peeling off at the check that settles
/// it into its card, then what all the stops ended as, over 1h / 24h / 7d.
/// Tap a card, or one of its outcome lines, for a real decision from it;
/// its Why drills down to each step's record.
struct RoutingView: View {
    let client: Client
    /// Opens a `buzz://` link (its channel tab, its thread).
    let open: (String) -> Void
    @Environment(Model.self) private var model
    @Environment(\.theme) private var theme
    @State private var window: RouteWindow = .day
    /// The card or outcome line sampled ("needs_you", "needs_you/ping"),
    /// and which of its decisions (newest first).
    @State private var picked: String?
    @State private var nth: UInt32 = 0
    @State private var sample: RouteDecision?
    @State private var why: RouteDecision?

    var body: some View {
        let _ = model.version
        let pipe = client.routePipeline(window: window)
        ZStack {
            overview(pipe)
                .opacity(why == nil ? 1 : 0)
                .allowsHitTesting(why == nil)
            if let d = why {
                WhyView(client: client, decision: d, open: open) {
                    withAnimation(.easeOut(duration: 0.2)) { why = nil }
                }
                .transition(.move(edge: .trailing))
            }
        }
        .clipped()
        .sheet(isPresented: Binding(get: { sample != nil }, set: { if !$0 { sample = nil; picked = nil } })) {
            if let d = sample {
                SampleCard(decision: d, box: pickLabel(pipe, picked),
                           open: { link in
                               sample = nil
                               picked = nil
                               open(link)
                           },
                           another: another,
                           why: {
                               sample = nil
                               picked = nil
                               withAnimation(.easeOut(duration: 0.2)) { why = d }
                           })
                    .presentationDetents([.height(330), .large])
                    .presentationDragIndicator(.visible)
                    .modifier(Themed())
            }
        }
    }

    private func overview(_ pipe: RoutePipeline) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                Text("Routing")
                    .font(.heading)
                    .foregroundStyle(theme.bright)
                    .padding(.top, 12)
                picker
                Text("Each colour is one group of stops. It runs down the band and peels off at the check that settles it, into its card. Width = number of stops. The number on an arrow is how many stops left there; the card says what the watcher did with them. Jev reads the agent's last message; Claude double-checks anything Jev calls a question.")
                    .font(.small)
                    .foregroundStyle(theme.muted)
                    .fixedSize(horizontal: false, vertical: true)
                OutcomeLegend(items: pipe.outcomes, long: true)
                if pipe.total == 0 {
                    Text("no routing decisions in the last \(label(window))")
                        .font(.small)
                        .foregroundStyle(theme.muted)
                        .frame(maxWidth: .infinity)
                        .padding(.top, 40)
                } else {
                    PipelineDiagram(pipe: pipe, picked: picked, tap: pick)
                    Totals(pipe: pipe)
                }
                if let note = pipe.retiredNote {
                    Text(note)
                        .font(.small)
                        .foregroundStyle(theme.faint)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            .padding(.horizontal, 16)
            .padding(.bottom, 40)
        }
    }

    /// 1h / 24h / 7d.
    private var picker: some View {
        HStack(spacing: 4) {
            ForEach([RouteWindow.hour, .day, .week], id: \.self) { w in
                let on = w == window
                Button {
                    withAnimation(.easeOut(duration: 0.15)) { window = w }
                } label: {
                    Text(label(w))
                        .font(on ? .chipOn : .chip)
                        .foregroundStyle(on ? theme.onAccent : theme.muted)
                        .frame(maxWidth: .infinity, minHeight: 36)
                        .background(on ? theme.chipOn : Color.clear, in: RoundedRectangle(cornerRadius: 8))
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
        }
        .padding(4)
        .background(theme.chipOff, in: RoundedRectangle(cornerRadius: 11))
        .frame(maxWidth: 280)
        .frame(maxWidth: .infinity)
    }

    private func label(_ w: RouteWindow) -> String {
        switch w {
        case .hour: return "1h"
        case .day: return "24h"
        case .week: return "7d"
        }
    }

    /// "Claude: needs you", or "Claude: needs you · pinged you" for a line.
    private func pickLabel(_ pipe: RoutePipeline, _ key: String?) -> String {
        guard let key else { return "" }
        for g in pipe.gates {
            for c in g.cards {
                if c.key == key { return c.title }
                if let o = c.outcomes.first(where: { $0.key == key }) { return "\(c.title) · \(o.word)" }
            }
        }
        return ""
    }

    private func pick(_ key: String, _ count: UInt32, _ name: String) {
        guard count > 0 else {
            model.say("nothing in \(name) in the last \(label(window))")
            return
        }
        picked = key
        nth = 0
        sample = client.routeSample(window: window, key: key, n: 0)
    }

    /// The next decision from the same card or line, round again past the
    /// oldest.
    private func another() {
        guard let key = picked else { return }
        nth &+= 1
        let next = client.routeSample(window: window, key: key, n: nth)
        if next == sample {
            model.say("that's the only one")
        }
        sample = next
    }
}

/// The three ends of the Why chain: no action, a nudge to the driver, a ping
/// to you. A Why step takes the color of where it leads.
private enum FlowTone {
    case quiet, nudge, ping

    init(_ key: String) {
        switch key {
        case "out:no_action", "claude:not_a_question": self = .quiet
        case "out:ping_you", "claude:needs_you", "jev:asks_something": self = .ping
        default: self = key.hasSuffix(":other") ? .quiet : .nudge
        }
    }

    func color(_ theme: Theme) -> Color {
        switch self {
        case .quiet: return theme.faint
        case .nudge: return theme.routeNudge
        case .ping: return theme.routePing
        }
    }
}

private func flowColor(_ key: String, _ theme: Theme) -> Color {
    FlowTone(key).color(theme)
}

/// "#rrggbb" from the core (stream and outcome colours).
private func hexColor(_ s: String) -> Color {
    let v = UInt32(s.dropFirst(s.hasPrefix("#") ? 1 : 0), radix: 16) ?? 0x9AA3AD
    return Color(.sRGB, red: Double(v >> 16 & 0xFF) / 255, green: Double(v >> 8 & 0xFF) / 255,
                 blue: Double(v & 0xFF) / 255, opacity: 1)
}

/// Dot + glyph + word for each outcome: under the intro (`long`, no
/// counts) and under the totals bar (with counts).
private struct OutcomeLegend: View {
    let items: [RouteOutcome]
    let long: Bool
    @Environment(\.theme) private var theme

    var body: some View {
        HStack(spacing: 12) {
            ForEach(items, id: \.outcome) { o in
                HStack(spacing: 5) {
                    Circle().fill(hexColor(o.color)).frame(width: 8, height: 8)
                    if long {
                        Text("\(o.glyph) \(o.outcome == "nudge" ? "nudged the agent" : o.word)")
                            .foregroundStyle(theme.muted)
                    } else {
                        Text(o.glyph).foregroundStyle(theme.text)
                        Text("\(o.count)").font(.textBold).foregroundStyle(theme.bright)
                        Text(o.word).foregroundStyle(theme.muted)
                    }
                }
                .lineLimit(1)
                if !long && o.outcome != items.last?.outcome { Spacer(minLength: 0) }
            }
        }
        .font(.small)
        .minimumScaleFactor(0.8)
    }
}

/// "All N stops ended as": one bar in the three outcome colours, then the
/// counts.
private struct Totals: View {
    let pipe: RoutePipeline
    @Environment(\.theme) private var theme

    var body: some View {
        let total = CGFloat(max(pipe.total, 1))
        let shown = pipe.outcomes.filter { $0.count > 0 }
        VStack(alignment: .leading, spacing: 8) {
            Text("All \(pipe.total) stops ended as")
                .font(.name)
                .foregroundStyle(theme.bright)
            GeometryReader { geo in
                let room = geo.size.width - CGFloat(max(shown.count - 1, 0)) * 2
                HStack(spacing: 2) {
                    ForEach(shown, id: \.outcome) { o in
                        Rectangle()
                            .fill(hexColor(o.color))
                            .frame(width: max(2, room * CGFloat(o.count) / total))
                    }
                }
            }
            .frame(height: 12)
            .clipShape(RoundedRectangle(cornerRadius: 4))
            OutcomeLegend(items: pipe.outcomes, long: false)
        }
        .padding(.top, 8)
    }
}

/// The trunk and its cards, top to bottom. Every row draws the strips still
/// running down at a fixed x (`Strips`), so they line up from row to row;
/// a card's row also draws its own strip turning right into it.
private struct PipelineDiagram: View {
    let pipe: RoutePipeline
    let picked: String?
    let tap: (String, UInt32, String) -> Void
    @Environment(\.theme) private var theme

    var body: some View {
        let s = Strips(pipe: pipe)
        VStack(alignment: .leading, spacing: 0) {
            // stops seen
            HStack(spacing: 10) {
                Text("\(pipe.total)")
                    .font(.heading)
                    .foregroundStyle(.white)
                    .padding(.horizontal, 10)
                    .frame(minWidth: s.trunkWidth, minHeight: 44, alignment: .leading)
                    .background(hexColor("#4a5568"), in: RoundedRectangle(cornerRadius: 6))
                VStack(alignment: .leading, spacing: 2) {
                    Text("stops seen").font(.textBold).foregroundStyle(theme.bright)
                    Text("each time an agent went quiet").font(.tiny).foregroundStyle(theme.muted)
                }
            }
            ForEach(Array(pipe.gates.enumerated()), id: \.offset) { gi, g in
                // The check: a quiet caption beside the trunk.
                VStack(alignment: .leading, spacing: 2) {
                    Text(g.caption).font(.small).foregroundStyle(theme.muted)
                    if let z = g.zeros {
                        Text(z).font(.tiny).foregroundStyle(theme.faint)
                    }
                }
                .fixedSize(horizontal: false, vertical: true)
                .padding(.vertical, 8)
                .padding(.leading, s.trunkWidth + 8)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(alignment: .leading) { s.trunk(from: s.start(ofGate: gi)) }
                ForEach(g.cards, id: \.key) { c in
                    let i = s.index(of: c.key)
                    CardView(card: c, picked: picked, tap: tap)
                        .padding(.leading, s.cardX)
                        .padding(.vertical, 6)
                        .frame(minHeight: s.bandTop + s.width(i) + 22, alignment: .top)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .background(alignment: .leading) { s.peel(i, card: c) }
                }
            }
        }
    }
}

/// Where each stream's strip sits on the trunk. The stream that leaves
/// first is on the right, nearest the cards, so every strip peels right
/// without crossing one still running down.
private struct Strips {
    let cards: [RouteCard]
    let lefts: [CGFloat]
    let widths: [CGFloat]
    let trunkWidth: CGFloat
    /// Cards before each gate (one more entry for past the last).
    let gateStarts: [Int]
    /// Where the cards start.
    var cardX: CGFloat { trunkWidth + 40 }
    /// How far down a card's row its strip turns right.
    let bandTop: CGFloat = 22
    private static let gap: CGFloat = 1.5

    init(pipe: RoutePipeline) {
        cards = pipe.gates.flatMap { $0.cards }
        var starts = [0]
        for g in pipe.gates { starts.append(starts[starts.count - 1] + g.cards.count) }
        gateStarts = starts
        let total = CGFloat(max(pipe.total, 1))
        // About 90 points of trunk for the stops, at least 3 per strip.
        let k = min(0.8, 90 / total)
        widths = cards.map { max(3, CGFloat($0.count) * k) }
        var x: CGFloat = 0
        var ls = Array(repeating: CGFloat(0), count: cards.count)
        for i in cards.indices.reversed() {
            ls[i] = x
            x += widths[i] + Strips.gap
        }
        lefts = ls
        trunkWidth = max(x - Strips.gap, 0)
    }

    func index(of key: String) -> Int {
        cards.firstIndex(where: { $0.key == key }) ?? cards.count
    }

    /// The first card at or after gate `g`: the strips still running there.
    func start(ofGate g: Int) -> Int {
        gateStarts[min(g, gateStarts.count - 1)]
    }

    func width(_ i: Int) -> CGFloat { i < widths.count ? widths[i] : 0 }

    /// The strips from card `from` on, straight down the whole row.
    func trunk(from: Int) -> some View {
        Canvas { ctx, size in
            for i in cards.indices where i >= from {
                ctx.fill(Path(CGRect(x: lefts[i], y: 0, width: widths[i], height: size.height)),
                         with: .color(hexColor(cards[i].color)))
            }
        }
        .frame(width: cardX)
    }

    /// Card `i`'s row: the strips still running past it, and its own strip
    /// coming down, turning right and ending in an arrow at the card with
    /// the count on it.
    func peel(_ i: Int, card: RouteCard) -> some View {
        let color = hexColor(card.color)
        let ink = hexColor(card.ink)
        let (l, h) = (lefts[i], widths[i])
        let r = l + h
        let yc = bandTop
        let xe = cardX
        return ZStack(alignment: .topLeading) {
            trunk(from: i + 1)
            Canvas { ctx, _ in
                let r1: CGFloat = 6
                let r2 = r1 + h
                var p = Path()
                p.move(to: CGPoint(x: l, y: 0))
                p.addLine(to: CGPoint(x: r, y: 0))
                p.addLine(to: CGPoint(x: r, y: yc - r1))
                p.addQuadCurve(to: CGPoint(x: r + r1, y: yc), control: CGPoint(x: r, y: yc))
                p.addLine(to: CGPoint(x: xe - 6, y: yc))
                p.addLine(to: CGPoint(x: xe - 6, y: yc + h))
                p.addLine(to: CGPoint(x: l + r2, y: yc + h))
                p.addQuadCurve(to: CGPoint(x: l, y: yc + h - r2), control: CGPoint(x: l, y: yc + h))
                p.closeSubpath()
                ctx.fill(p, with: .color(color))
                let mid = yc + h / 2
                var a = Path()
                a.move(to: CGPoint(x: xe - 8, y: mid - 6))
                a.addLine(to: CGPoint(x: xe - 1, y: mid))
                a.addLine(to: CGPoint(x: xe - 8, y: mid + 6))
                a.closeSubpath()
                ctx.fill(a, with: .color(color))
            }
            .frame(width: cardX)
            // The count: inside a wide band, white; above a thin one, in
            // the stream's colour darkened for contrast.
            Text("\(card.count) →")
                .font(.textBold)
                .foregroundStyle(h >= 20 ? Color.white : ink)
                .frame(width: xe - 12, alignment: .trailing)
                .offset(y: h >= 20 ? yc + h / 2 - 10 : yc - 20)
        }
        .frame(width: cardX, alignment: .topLeading)
    }
}

/// One stream's card: title, plain line, then its outcome lines (coloured
/// dot, glyph, word, count, sub-lines). Tap the card or a line to sample.
private struct CardView: View {
    let card: RouteCard
    let picked: String?
    let tap: (String, UInt32, String) -> Void
    @Environment(\.theme) private var theme

    var body: some View {
        let color = hexColor(card.color)
        let on = picked == card.key
        VStack(alignment: .leading, spacing: 6) {
            VStack(alignment: .leading, spacing: 2) {
                Text(card.title).font(.name).foregroundStyle(theme.bright)
                Text(card.why).font(.tiny).foregroundStyle(theme.muted)
            }
            ForEach(card.outcomes, id: \.key) { o in
                Button { tap(o.key, o.count, "\(card.title) · \(o.word)") } label: {
                    outcomeLine(o)
                }
                .buttonStyle(.plain)
            }
        }
        .fixedSize(horizontal: false, vertical: true)
        .padding(.leading, 18)
        .padding(.trailing, 10)
        .padding(.vertical, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(theme.card, in: RoundedRectangle(cornerRadius: 10))
        .overlay(alignment: .leading) {
            Rectangle().fill(color).frame(width: 5).padding(.vertical, 8)
        }
        .overlay(RoundedRectangle(cornerRadius: 10).stroke(color, lineWidth: on ? 3 : 1.5))
        .contentShape(Rectangle())
        .onTapGesture { tap(card.key, card.count, card.title) }
    }

    private func outcomeLine(_ o: RouteOutcome) -> some View {
        let on = picked == o.key
        return HStack(alignment: .firstTextBaseline, spacing: 6) {
            Circle().fill(hexColor(o.color)).frame(width: 8, height: 8)
            Text(o.glyph).font(.small).foregroundStyle(theme.text).frame(minWidth: 16)
            VStack(alignment: .leading, spacing: 1) {
                (Text("\(o.word) ") + Text("\(o.count)").bold())
                    .font(.small)
                    .foregroundStyle(theme.bright)
                ForEach(o.notes, id: \.self) { n in
                    Text(n).font(.tiny).foregroundStyle(theme.muted)
                }
            }
            .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
        }
        .padding(.vertical, 2)
        .padding(.horizontal, 4)
        .background(on ? hexColor(o.color).opacity(0.18) : Color.clear, in: RoundedRectangle(cornerRadius: 6))
        .contentShape(Rectangle())
    }
}

/// A sampled decision from a card or outcome line: where and what the watcher stopped
/// on, how it routed, what it posted.
private struct SampleCard: View {
    let decision: RouteDecision
    /// The card (or line) it was sampled from.
    let box: String
    let open: (String) -> Void
    let another: () -> Void
    let why: () -> Void
    @Environment(\.theme) private var theme

    var body: some View {
        let d = decision
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 8) {
                Text(([d.channelLabel, d.whoName, clock(d.at)].filter { !$0.isEmpty }).joined(separator: " · "))
                    .font(.small)
                    .foregroundStyle(theme.muted)
                    .lineLimit(1)
                Spacer()
                if !box.isEmpty {
                    Text(box).font(.tiny).foregroundStyle(theme.faint).lineLimit(1)
                }
            }
            Button(action: why) {
                VStack(alignment: .leading, spacing: 8) {
                    Text(d.text.isEmpty ? d.reason : "“\(d.text)”")
                        .font(.name)
                        .foregroundStyle(theme.bright)
                        .lineLimit(3)
                        .multilineTextAlignment(.leading)
                    if let j = d.jev {
                        line("Jev", j, flowColor(d.path.first(where: { $0.hasPrefix("jev:") }) ?? "", theme))
                    }
                    if d.model != nil {
                        line("Claude", d.reason.isEmpty ? (d.model ?? "") : d.reason, theme.text)
                    }
                    line("Posted", posted(d), theme.text)
                    Label("why", systemImage: "chevron.right")
                        .labelStyle(TrailingIcon())
                        .font(.small)
                        .foregroundStyle(theme.link)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            HStack(spacing: 12) {
                action("Open thread", "arrow.up.forward.square") {
                    if let l = d.link { open(l) }
                }
                .disabled(d.link == nil)
                .opacity(d.link == nil ? 0.4 : 1)
                action("Sample another", "arrow.triangle.2.circlepath", another)
            }
        }
        .padding(22)
        .frame(maxHeight: .infinity, alignment: .top)
        .background(theme.sheet)
    }

    private func line(_ name: String, _ value: String, _ color: Color) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Text("\(name):").foregroundStyle(theme.muted).frame(width: 70, alignment: .leading)
            Text(value).foregroundStyle(color).lineLimit(2)
        }
        .font(.small)
    }

    private func action(_ title: String, _ icon: String, _ run: @escaping () -> Void) -> some View {
        Button(action: run) {
            Label(title, systemImage: icon)
                .font(.button)
                .foregroundStyle(theme.text)
                .frame(maxWidth: .infinity, minHeight: 48)
                .background(theme.card, in: RoundedRectangle(cornerRadius: 10))
                .overlay(RoundedRectangle(cornerRadius: 10).stroke(theme.rule, lineWidth: 1))
        }
        .buttonStyle(.plain)
    }
}

/// What a decision posted, in words.
private func posted(_ d: RouteDecision) -> String {
    var s: String
    switch d.outcome {
    case "nudge": s = "nudge to the driver"
    case "ping": s = "ping to you"
    case "none": s = "nothing"
    default: s = d.outcome.replacingOccurrences(of: "_", with: " ")
    }
    if d.posts.count > 1 { s += " (\(d.posts.count))" }
    if let st = d.state, !st.isEmpty { s += " · " + st.replacingOccurrences(of: "_", with: " ") }
    return s
}

private func clock(_ at: UInt64) -> String {
    Date(timeIntervalSince1970: TimeInterval(at)).formatted(date: .omitted, time: .shortened)
}

/// Text first, then the icon.
private struct TrailingIcon: LabelStyle {
    func makeBody(configuration: Configuration) -> some View {
        HStack(spacing: 4) {
            configuration.title
            configuration.icon
        }
    }
}

/// One decision's Why: what the watcher stopped on, then each step down
/// the chain, each opening its record: the stop message, the channel's
/// checklist (where the watch record shows), the routing record's fields,
/// the nudge or ping it posted.
private struct WhyView: View {
    let client: Client
    let decision: RouteDecision
    let open: (String) -> Void
    let back: () -> Void
    @Environment(\.theme) private var theme
    @State private var shown: String?
    @State private var checklist = false

    var body: some View {
        let d = decision
        let steps = routeWhy(decision: d)
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                HStack(spacing: 12) {
                    Button(action: back) {
                        Image(systemName: "arrow.left")
                            .font(.system(size: 15, weight: .semibold))
                            .foregroundStyle(theme.text)
                            .frame(width: 38, height: 38)
                            .background(theme.idle, in: Circle())
                            .overlay(Circle().stroke(theme.rule, lineWidth: 1))
                    }
                    .buttonStyle(.plain)
                    Text("Why")
                        .font(.heading)
                        .foregroundStyle(theme.bright)
                }
                .padding(.top, 12)
                header(d)
                ForEach(Array(steps.enumerated()), id: \.offset) { _, s in
                    step(s, d)
                    if s.title == "jev" {
                        notTaken(s.key)
                    }
                }
                Label("Tap a step to open its record", systemImage: "info.circle")
                    .font(.small)
                    .foregroundStyle(theme.faint)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 12)
                    .overlay(RoundedRectangle(cornerRadius: 10).stroke(theme.rule, lineWidth: 1))
            }
            .padding(.horizontal, 22)
            .padding(.bottom, 40)
        }
        .background(theme.canvas)
        .sheet(isPresented: $checklist) {
            ChecklistView(client: client, channel: d.channel)
                .presentationDetents([.medium, .large])
                .presentationDragIndicator(.visible)
                .modifier(Themed())
        }
    }

    private func header(_ d: RouteDecision) -> some View {
        HStack(alignment: .top, spacing: 14) {
            Image(systemName: "cpu")
                .font(.system(size: 20))
                .foregroundStyle(theme.text)
                .frame(width: 46, height: 46)
                .background(theme.idle, in: RoundedRectangle(cornerRadius: 10))
            VStack(alignment: .leading, spacing: 4) {
                HStack {
                    // The stop message's author, when we hold the message.
                    Text(d.whoName.isEmpty ? d.channelLabel : d.whoName)
                        .font(.name)
                        .foregroundStyle(theme.bright)
                    Spacer()
                    Text(ago(d.at)).font(.small).foregroundStyle(theme.faint)
                }
                if !d.whoName.isEmpty {
                    Text(d.channelLabel).font(.small).foregroundStyle(theme.muted)
                }
                Text(d.text.isEmpty ? d.reason : d.text)
                    .font(.text)
                    .foregroundStyle(theme.text)
                    .lineLimit(4)
            }
        }
        .padding(14)
        .background(theme.card, in: RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(theme.rule, lineWidth: 1))
        .contentShape(Rectangle())
        .onTapGesture { if let l = d.link { open(l) } }
    }

    private func step(_ s: RouteStep, _ d: RouteDecision) -> some View {
        let color = flowColor(s.key, theme)
        // The branches taken get their color; the first two steps are plain.
        let tinted = FlowTone(s.key) != .quiet && s.key != "stops_seen" && s.key != "running"
        let opens = s.link != nil || s.title == "checklist" || s.eventId != nil
        return VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .top, spacing: 14) {
                Image(systemName: icon(s.title))
                    .font(.system(size: 20))
                    .foregroundStyle(theme.text)
                    .frame(width: 46, height: 46)
                    .background(theme.idle, in: RoundedRectangle(cornerRadius: 10))
                VStack(alignment: .leading, spacing: 6) {
                    Text(title(s.title))
                        .font(.name)
                        .foregroundStyle(theme.bright)
                    Text(s.label)
                        .font(.small)
                        .foregroundStyle(tinted ? color : theme.text)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 4)
                        .background((tinted ? color : theme.muted).opacity(0.2), in: RoundedRectangle(cornerRadius: 8))
                    if !s.detail.isEmpty {
                        Text(s.detail)
                            .font(.mono)
                            .foregroundStyle(theme.muted)
                            .lineLimit(3)
                    }
                }
                Spacer(minLength: 0)
                if opens {
                    Image(systemName: shown == s.key ? "chevron.down" : "chevron.right")
                        .foregroundStyle(theme.faint)
                        .padding(.top, 14)
                }
            }
            if s.title == "result", let l = s.link {
                Button { open(l) } label: {
                    Label(openPost(s.key), systemImage: "arrow.up.forward.square")
                        .font(.label)
                        .foregroundStyle(theme.text)
                        .padding(.horizontal, 14)
                        .padding(.vertical, 8)
                        .background(theme.idle, in: RoundedRectangle(cornerRadius: 8))
                }
                .buttonStyle(.plain)
                .padding(.leading, 60)
            }
            if shown == s.key, let id = s.eventId {
                record(id, d).padding(.leading, 60)
            }
        }
        .padding(14)
        .background(theme.card, in: RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(theme.rule, lineWidth: 1))
        .contentShape(Rectangle())
        .onTapGesture { tap(s) }
    }

    /// A message opens in its thread; the checklist step opens the
    /// channel's checklist; the routing record shows its fields here.
    private func tap(_ s: RouteStep) {
        if let l = s.link {
            open(l)
        } else if s.title == "checklist" {
            checklist = true
        } else if s.eventId != nil {
            withAnimation(.easeOut(duration: 0.15)) { shown = shown == s.key ? nil : s.key }
        }
    }

    private func record(_ id: String, _ d: RouteDecision) -> some View {
        let fields: [(String, String)] = [
            ("record", id),
            ("trigger", d.trigger),
            ("stage", d.stage),
            ("jev", d.jev ?? "–"),
            ("claude", d.model ?? "–"),
            ("outcome", d.outcome),
            ("reason", d.reason),
        ]
        return VStack(alignment: .leading, spacing: 3) {
            ForEach(fields, id: \.0) { k, v in
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(k).foregroundStyle(theme.faint).frame(width: 64, alignment: .leading)
                    Text(v).foregroundStyle(theme.code).lineLimit(k == "record" ? 1 : 3).truncationMode(.middle)
                }
            }
        }
        .font(.tiny)
        .textSelection(.enabled)
    }

    /// The Jev streams it didn't take, faint, as in the mock.
    private func notTaken(_ key: String) -> some View {
        let all = [("jev:keeps_going", "keeps going"), ("jev:waiting_on_a_bot", "waiting on a bot"),
                   ("jev:finished", "finished"), ("jev:asks_something", "asks something")]
        return VStack(alignment: .leading, spacing: 8) {
            ForEach(all.filter { $0.0 != key }, id: \.0) { _, name in
                HStack(spacing: 10) {
                    Circle().stroke(theme.faint, lineWidth: 1.5).frame(width: 14, height: 14)
                    Text(name).font(.small).foregroundStyle(theme.faint)
                }
            }
        }
        .padding(.leading, 34)
    }

    private func icon(_ title: String) -> String {
        switch title {
        case "watcher saw it": return "eye"
        case "checklist": return "checklist"
        case "jev": return "ellipsis.bubble"
        case "claude check": return "sparkles"
        default: return "doc.text"
        }
    }

    private func title(_ t: String) -> String {
        switch t {
        case "jev": return "Jev"
        case "claude check": return "Claude check"
        default: return t.prefix(1).uppercased() + t.dropFirst()
        }
    }

    private func openPost(_ key: String) -> String {
        switch key {
        case "out:ping_you": return "Open ping"
        case "out:nudge_the_driver": return "Open nudge"
        default: return "Open post"
        }
    }

    private func ago(_ at: UInt64) -> String {
        let now = UInt64(Date().timeIntervalSince1970)
        let s = at > now ? 0 : now - at
        if s < 60 { return "\(s)s ago" }
        if s < 3600 { return "\(s / 60) min ago" }
        if s < 86_400 { return "\(s / 3600)h ago" }
        return "\(s / 86_400)d ago"
    }
}
