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

/// How the watcher routed every stop it judged (`buzz_vim_core::routing`):
/// stops seen, running, Jev's stream, the Claude check (asks only) and the
/// outcome, as layered boxes with the bands between them, over 1h / 24h /
/// 7d. Tap a box for a real decision through it; its Why drills down to
/// each step's record.
struct RoutingView: View {
    let client: Client
    /// Opens a `buzz://` link (its channel tab, its thread).
    let open: (String) -> Void
    @Environment(Model.self) private var model
    @Environment(\.theme) private var theme
    @State private var window: RouteWindow = .day
    /// The box sampled, and which of its decisions (newest first).
    @State private var picked: String?
    @State private var nth: UInt32 = 0
    @State private var sample: RouteDecision?
    @State private var why: RouteDecision?

    var body: some View {
        let _ = model.version
        let flow = client.routeFlow(window: window)
        ZStack {
            overview(flow)
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
                SampleCard(decision: d, box: flow.nodes.first(where: { $0.key == picked })?.label ?? "",
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

    private func overview(_ flow: RouteFlow) -> some View {
        let seen = flow.nodes.first(where: { $0.layer == 0 })?.count ?? 0
        return ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                Text("Routing")
                    .font(.heading)
                    .foregroundStyle(theme.bright)
                    .padding(.top, 12)
                picker
                if seen == 0 {
                    Text("no routing decisions in the last \(label(window))")
                        .font(.small)
                        .foregroundStyle(theme.muted)
                        .frame(maxWidth: .infinity)
                        .padding(.top, 40)
                } else {
                    FlowDiagram(flow: flow, picked: picked, tap: pick)
                    Text("tap a box for a real decision through it")
                        .font(.small)
                        .foregroundStyle(theme.faint)
                        .frame(maxWidth: .infinity)
                }
            }
            .padding(.horizontal, 22)
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

    private func pick(_ n: RouteNode) {
        guard n.count > 0 else {
            model.say("nothing went through \(n.label) in the last \(label(window))")
            return
        }
        picked = n.key
        nth = 0
        sample = client.routeSample(window: window, key: n.key, n: 0)
    }

    /// The next decision through the same box, round again past the oldest.
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

/// The three ends: no action, a nudge to the driver, a ping to you. A box
/// (and the band into it) takes the color of where it leads.
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

/// Where each box sits, for the bands drawn behind them.
private struct BoxAnchors: PreferenceKey {
    static let defaultValue: [String: Anchor<CGRect>] = [:]
    static func reduce(value: inout [String: Anchor<CGRect>], nextValue: () -> [String: Anchor<CGRect>]) {
        value.merge(nextValue()) { $1 }
    }
}

/// The layered boxes, top to bottom, with a band for every pair of boxes
/// decisions went between, as wide as the share that went. The left lane
/// stays clear for the no-ops falling straight to "no action".
private struct FlowDiagram: View {
    let flow: RouteFlow
    let picked: String?
    let tap: (RouteNode) -> Void
    @Environment(\.theme) private var theme
    @State private var width: CGFloat = 0

    /// Each layer's left inset, as a share of the width.
    private let insets: [CGFloat] = [0, 0.26, 0.26, 0.5, 0]

    var body: some View {
        VStack(alignment: .leading, spacing: 34) {
            ForEach(0..<flow.layers.count, id: \.self) { i in
                let nodes = flow.nodes.filter { $0.layer == UInt32(i) }
                HStack(alignment: .top, spacing: 6) {
                    ForEach(nodes, id: \.key) { n in
                        box(n, wide: nodes.count == 1)
                    }
                }
                .padding(.leading, width * (i < insets.count ? insets[i] : 0))
            }
        }
        .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { width = $0 }
        .backgroundPreferenceValue(BoxAnchors.self) { anchors in
            GeometryReader { geo in
                let rects = anchors.mapValues { geo[$0] }
                Canvas { ctx, _ in draw(ctx, rects) }
            }
        }
        .padding(.top, 4)
    }

    private func box(_ n: RouteNode, wide: Bool) -> some View {
        let color = flowColor(n.key, theme)
        let end = n.layer == 4
        let on = n.key == picked
        return Button { tap(n) } label: {
            Group {
                if wide {
                    HStack {
                        Text(n.label).font(.label).foregroundStyle(theme.bright)
                        Spacer()
                        Text("\(n.count)").font(.textBold).foregroundStyle(theme.bright)
                    }
                    .padding(.horizontal, 14)
                } else {
                    VStack(spacing: 2) {
                        Text(n.label)
                            .font(end ? .chipOn : .tiny)
                            .foregroundStyle(end ? theme.bright : theme.text)
                            .multilineTextAlignment(.center)
                            .lineLimit(2)
                            .minimumScaleFactor(0.8)
                        Text("\(n.count)")
                            .font(end ? .heading : .textBold)
                            .foregroundStyle(end ? theme.bright : (n.count > 0 ? color : theme.faint))
                    }
                    .padding(.horizontal, 4)
                }
            }
            .frame(maxWidth: .infinity, minHeight: end ? 64 : 48)
            .background(end ? color.opacity(0.22) : theme.card, in: RoundedRectangle(cornerRadius: 10))
            .background(theme.canvas, in: RoundedRectangle(cornerRadius: 10))
            .overlay(
                RoundedRectangle(cornerRadius: 10)
                    .stroke(on ? color : (end ? color.opacity(0.6) : theme.rule), lineWidth: on ? 2 : 1)
            )
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .anchorPreference(key: BoxAnchors.self, value: .bounds) { [n.key: $0] }
    }

    /// Each band leaves its box's bottom and enters the next one's top,
    /// as straight as it can (over the box at its other end), side by side
    /// in the order of where they go (or come from), kept inside the box.
    private func draw(_ ctx: GraphicsContext, _ rects: [String: CGRect]) {
        let total = CGFloat(max(flow.nodes.first(where: { $0.layer == 0 })?.count ?? 1, 1))
        let widest: CGFloat = 46
        let edges = flow.edges.filter { rects[$0.from] != nil && rects[$0.to] != nil && $0.count > 0 }
        let thick = { (e: RouteEdge) in max(2, widest * CGFloat(e.count) / total) }
        var outX: [Int: CGFloat] = [:]
        var inX: [Int: CGFloat] = [:]
        // true: the bands leaving each box; false: those entering it.
        for side in [true, false] {
            let groups = Dictionary(grouping: edges.indices) { side ? edges[$0].from : edges[$0].to }
            for (node, idx) in groups {
                guard let r = rects[node] else { continue }
                let other = { (i: Int) in rects[side ? edges[i].to : edges[i].from]?.midX ?? 0 }
                let sorted = idx.sorted { other($0) < other($1) }
                var xs: [CGFloat] = []
                var end = r.minX
                for i in sorted {
                    let w = thick(edges[i])
                    let want = min(max(other(i) - w / 2, r.minX), r.maxX - w)
                    xs.append(max(want, end))
                    end = xs[xs.count - 1] + w
                }
                // Past the right edge: the whole group slides back in.
                let over = max(0, min(end - r.maxX, (xs.first ?? r.minX) - r.minX))
                for (k, i) in sorted.enumerated() {
                    if side { outX[i] = xs[k] - over } else { inX[i] = xs[k] - over }
                }
            }
        }
        for i in edges.indices {
            let e = edges[i]
            guard let a = rects[e.from], let b = rects[e.to], let x0 = outX[i], let x1 = inX[i] else { continue }
            let w = thick(e)
            let (y0, y1) = (a.maxY, b.minY)
            let mid = (y0 + y1) / 2
            var p = Path()
            p.move(to: CGPoint(x: x0, y: y0))
            p.addCurve(to: CGPoint(x: x1, y: y1), control1: CGPoint(x: x0, y: mid), control2: CGPoint(x: x1, y: mid))
            p.addLine(to: CGPoint(x: x1 + w, y: y1))
            p.addCurve(to: CGPoint(x: x0 + w, y: y0), control1: CGPoint(x: x1 + w, y: mid),
                       control2: CGPoint(x: x0 + w, y: mid))
            p.closeSubpath()
            ctx.fill(p, with: .color(flowColor(e.to, theme).opacity(0.45)))
        }
    }
}

/// A sampled decision through a box: where and what the watcher stopped
/// on, how it routed, what it posted.
private struct SampleCard: View {
    let decision: RouteDecision
    /// The box it was sampled from.
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
