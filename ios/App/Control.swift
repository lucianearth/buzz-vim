import Charts
import SwiftUI

/// The Control page: pinned last, after Routing, never in the switcher.
enum Control {
    static let id = "control"
}

/// Where the Control page is: the overview, a topic's days, or a note.
private enum ControlPlace: Hashable {
    case topic(String)
    case note(String)
}

/// The #control channel as an ops cockpit (`buzz_vim_core::control`, style
/// B): open requests with a button per declared action, a tile per topic
/// with its status and 14-day sparkline, and the newest hooks note's gauges
/// against their baselines. A tile opens its days, a day its note.
struct ControlView: View {
    let client: Client
    /// Opens a `buzz://` link (its channel tab, its thread).
    let open: (String) -> Void
    @Environment(Model.self) private var model
    @Environment(\.theme) private var theme
    /// Back goes one step along this.
    @State private var path: [ControlPlace] = []
    @State private var asked = false

    var body: some View {
        let _ = model.version
        ZStack {
            overview(client.controlOverview())
                .opacity(path.isEmpty ? 1 : 0)
                .allowsHitTesting(path.isEmpty)
            ForEach(Array(path.enumerated()), id: \.offset) { i, place in
                Group {
                    switch place {
                    case .topic(let t):
                        TopicDays(client: client, topic: t, back: back) { go(.note($0)) }
                    case .note(let id):
                        NotePage(client: client, id: id, open: open, back: back)
                    }
                }
                .background(theme.canvas)
                .opacity(i == path.count - 1 ? 1 : 0)
                .allowsHitTesting(i == path.count - 1)
                .transition(.move(edge: .trailing))
            }
        }
        .clipped()
        .onAppear {
            // Once per page; pull down to read it again.
            if !asked {
                asked = true
                client.openControl()
            }
        }
    }

    private func go(_ p: ControlPlace) {
        withAnimation(.easeOut(duration: 0.2)) { path.append(p) }
    }

    private func back() {
        withAnimation(.easeOut(duration: 0.2)) { _ = path.popLast() }
    }

    private func overview(_ o: ControlOverview) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                if !o.loaded {
                    Text("reading #control…")
                        .font(.small)
                        .foregroundStyle(theme.muted)
                        .frame(maxWidth: .infinity)
                        .padding(.top, 40)
                } else if let e = o.error {
                    VStack(spacing: 10) {
                        Text("couldn't read #control: \(e)")
                            .font(.small)
                            .foregroundStyle(theme.page)
                        Button("try again") { client.openControl() }
                            .font(.button)
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.top, 40)
                }
                if !o.requests.isEmpty {
                    RequestStrip(client: client, requests: o.requests, more: o.moreRequests) { go(.note($0)) }
                }
                TileGrid(tiles: o.tiles) { go(.topic($0)) }
                if let hooks = o.hooks {
                    Button { go(.note(hooks)) } label: {
                        HStack {
                            Text(o.hooksLabel)
                            Spacer()
                            Text("note ›")
                        }
                        .font(.small)
                        .foregroundStyle(theme.faint)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .padding(.top, 4)
                    GaugeGrid(gauges: o.gauges)
                        .contentShape(Rectangle())
                        .onTapGesture { go(.note(hooks)) }
                }
                if !o.seats.isEmpty {
                    SeatsLine(seats: o.seats)
                }
            }
            .padding(.horizontal, 16)
            .padding(.top, 8)
            .padding(.bottom, 40)
        }
        .refreshable { client.openControl() }
    }
}

// MARK: - colors

extension Theme {
    func health(_ h: ControlHealth) -> Color {
        switch h {
        case .ok: return ok
        case .warn: return warn
        case .page: return page
        case .noNote: return faint
        }
    }

    /// A delta's color: green better, amber a flag, red a page; a plain
    /// one draws in the data cyan, not a status color.
    func delta(_ t: DeltaTone) -> Color {
        switch t {
        case .plain: return data
        case .better: return ok
        case .flag: return warn
        case .page: return page
        }
    }

    /// A day's status word ("ok", "partial", "held", "open").
    func status(_ s: String?) -> Color {
        switch s {
        case "ok", "applied", "clean", "closed": return ok
        case "dead", "failed", "unreadable": return page
        case nil: return faint
        default: return warn
        }
    }
}

/// A card: the page's raised surface.
private struct Card: ViewModifier {
    var stroke: Color? = nil
    @Environment(\.theme) private var theme

    func body(content: Content) -> some View {
        content
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(theme.card, in: RoundedRectangle(cornerRadius: 14))
            .overlay(RoundedRectangle(cornerRadius: 14).stroke(stroke ?? theme.rule, lineWidth: 1))
    }
}

// MARK: - open requests

/// The amber strip on top: each open request, what it asks, a button per
/// declared action (its emoji is the vote), the approvers and its age.
private struct RequestStrip: View {
    let client: Client
    let requests: [ControlRequest]
    let more: UInt32
    let openNote: (String) -> Void
    @Environment(\.theme) private var theme

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("OPEN REQUESTS · \(requests.count + Int(more))")
                .font(.textBold)
                .foregroundStyle(theme.warn)
            ForEach(requests, id: \.id) { r in
                RequestBlock(client: client, request: r, openNote: openNote)
            }
            if more > 0 {
                Text("+\(more) more open: on their topic tiles")
                    .font(.small)
                    .foregroundStyle(theme.faint)
            }
        }
        .padding(.leading, 6)
        .modifier(Card())
        .overlay(alignment: .leading) {
            UnevenRoundedRectangle(topLeadingRadius: 14, bottomLeadingRadius: 14)
                .fill(theme.warn)
                .frame(width: 5)
        }
    }
}

/// One request: its line, its buttons, who may vote and how old it is.
/// `openNote` nil when it is already the note on screen.
private struct RequestBlock: View {
    let client: Client
    let request: ControlRequest
    var openNote: ((String) -> Void)? = nil
    @Environment(Model.self) private var model
    @Environment(\.theme) private var theme

    var body: some View {
        let r = request
        VStack(alignment: .leading, spacing: 8) {
            VStack(alignment: .leading, spacing: 4) {
                Text("\(r.topic) · \(r.day) · \(r.source)")
                    .font(.small)
                    .foregroundStyle(theme.muted)
                    .lineLimit(1)
                Text(r.line)
                    .font(.text)
                    .foregroundStyle(theme.text)
                    .lineLimit(3)
            }
            .contentShape(Rectangle())
            .onTapGesture { openNote?(r.id) }
            if r.status == "open" {
                // Wraps to a second row when the actions don't fit.
                ViewThatFits(in: .horizontal) {
                    HStack(spacing: 10) { buttons(r) }
                    VStack(alignment: .leading, spacing: 8) { buttons(r) }
                }
            } else {
                Text(r.status + (r.outcome.map { " (\($0))" } ?? ""))
                    .font(.small)
                    .foregroundStyle(theme.status(r.status))
            }
            Text(footer(r))
                .font(.small)
                .foregroundStyle(theme.faint)
                .lineLimit(1)
        }
    }

    @ViewBuilder
    private func buttons(_ r: ControlRequest) -> some View {
        ForEach(r.actions, id: \.index) { a in
            ActionButton(action: a, primary: a.index == 0) { vote(r, a) }
        }
    }

    private func footer(_ r: ControlRequest) -> String {
        var parts: [String] = []
        if !r.approvers.isEmpty {
            parts.append("approver " + r.approvers.joined(separator: ", "))
        }
        parts.append(r.age)
        return parts.joined(separator: " · ")
    }

    /// A tap sends the action's emoji as a reaction, or says why it can't.
    private func vote(_ r: ControlRequest, _ a: ControlAction) {
        if let why = a.blocked {
            model.say(why)
            return
        }
        do {
            try client.controlVote(note: r.id, action: a.index)
            model.say("voting \(a.name) \(a.emoji)…")
        } catch let BuzzError.Failed(message) {
            model.say(message)
        } catch {
            model.say(error.localizedDescription)
        }
    }
}

/// An action as a button: the first filled green, the rest outlined amber;
/// dim when you can't vote it (a tap still says why).
private struct ActionButton: View {
    let action: ControlAction
    let primary: Bool
    let tap: () -> Void
    @Environment(\.theme) private var theme

    var body: some View {
        let a = action
        let color = primary ? theme.ok : theme.warn
        Button(action: tap) {
            HStack(spacing: 8) {
                Text(a.emoji)
                Text(a.name.capitalized)
                    .font(.button)
                if a.votes > 0 {
                    Text(a.mine ? "\(a.votes) ✓" : "\(a.votes)")
                        .font(.small)
                        .padding(.horizontal, 6)
                        .padding(.vertical, 1)
                        .background(primary ? theme.onAccent.opacity(0.15) : color.opacity(0.15),
                                    in: Capsule())
                }
            }
            .foregroundStyle(primary ? theme.onAccent : color)
            .padding(.horizontal, 18)
            .frame(minHeight: 44)
            .background(primary ? color : Color.clear, in: RoundedRectangle(cornerRadius: 10))
            .overlay(RoundedRectangle(cornerRadius: 10).stroke(color, lineWidth: primary ? 0 : 1.5))
            .opacity(a.blocked == nil ? 1 : 0.5)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("\(a.name), \(a.votes) votes")
    }
}

// MARK: - topic tiles

private struct TileGrid: View {
    let tiles: [ControlTile]
    let tap: (String) -> Void

    var body: some View {
        LazyVGrid(columns: [GridItem(.flexible(), spacing: 10), GridItem(.flexible(), spacing: 10)],
                  spacing: 10) {
            ForEach(tiles, id: \.topic) { t in
                TileView(tile: t)
                    .contentShape(Rectangle())
                    .onTapGesture { tap(t.topic) }
            }
        }
    }
}

/// A topic: its name and status glyph, the worst note's one-liner on its
/// newest day, the key metric's 14 days, and the newest value.
private struct TileView: View {
    let tile: ControlTile
    @Environment(\.theme) private var theme

    var body: some View {
        let t = tile
        let color = theme.health(t.health)
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 6) {
                Text(t.topic)
                    .font(.chipOn)
                    .foregroundStyle(t.health == .noNote ? theme.muted : theme.bright)
                    .lineLimit(1)
                Spacer(minLength: 4)
                if t.open > 0 {
                    Text("\(t.open) open")
                        .font(.tiny)
                        .foregroundStyle(theme.warn)
                }
                Text(t.glyph)
                    .font(.textBold)
                    .foregroundStyle(color)
                Circle().fill(color).frame(width: 9, height: 9)
            }
            Text(t.line)
                .font(.small)
                .foregroundStyle(t.health == .noNote ? theme.faint : theme.text)
                .lineLimit(2, reservesSpace: true)
            Spark(values: t.spark, color: t.health == .noNote ? theme.faint : color, bars: true)
                .frame(height: 34)
            HStack {
                Text(t.metric)
                    .lineLimit(1)
                Spacer(minLength: 4)
                if let v = t.latest {
                    Text(v).foregroundStyle(theme.bright)
                }
            }
            .font(.tiny)
            .foregroundStyle(theme.faint)
            if let d = t.latestDay {
                Text(([String(d.dropFirst(5))] + t.sources).joined(separator: " · "))
                    .font(.tiny)
                    .foregroundStyle(theme.faint)
                    .lineLimit(1)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(theme.card, in: RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(t.open > 0 ? theme.warn.opacity(0.6) : theme.rule,
                                                           lineWidth: 1))
        .accessibilityElement(children: .combine)
    }
}

/// A sparkline over days (oldest first): bars, or a line with a soft fill.
/// A day without a value is a gap, never a zero.
private struct Spark: View {
    let values: [Double?]
    let color: Color
    var bars = false
    @Environment(\.theme) private var theme

    private struct Point: Identifiable {
        let id: Int
        let value: Double
        let run: Int
    }

    /// The days with a value, each with the run of consecutive days it's in.
    private var points: [Point] {
        var out: [Point] = []
        var run = 0
        var last = -2
        for (i, v) in values.enumerated() {
            guard let v else { continue }
            if i != last + 1 { run += 1 }
            last = i
            out.append(Point(id: i, value: v, run: run))
        }
        return out
    }

    var body: some View {
        let pts = points
        let top = max(pts.map(\.value).max() ?? 1, 1e-9)
        if pts.isEmpty {
            Rectangle()
                .fill(theme.rule)
                .frame(height: 1)
                .frame(maxHeight: .infinity, alignment: .bottom)
        } else {
            Chart {
                ForEach(pts) { p in
                    if bars {
                        // Days as categories, so every day keeps its slot.
                        BarMark(x: .value("day", String(p.id)), y: .value("value", p.value), width: .ratio(0.62))
                            .foregroundStyle(color.opacity(0.45 + 0.55 * Double(p.id + 1) / Double(values.count)))
                            .cornerRadius(2)
                    } else {
                        AreaMark(x: .value("day", Double(p.id)), y: .value("value", p.value),
                                 series: .value("run", p.run), stacking: .unstacked)
                            .foregroundStyle(LinearGradient(colors: [color.opacity(0.28), color.opacity(0)],
                                                            startPoint: .top, endPoint: .bottom))
                        LineMark(x: .value("day", Double(p.id)), y: .value("value", p.value),
                                 series: .value("run", p.run))
                            .foregroundStyle(color)
                            .lineStyle(StrokeStyle(lineWidth: 1.8, lineCap: .round, lineJoin: .round))
                        if pts.filter({ $0.run == p.run }).count == 1 {
                            PointMark(x: .value("day", Double(p.id)), y: .value("value", p.value))
                                .foregroundStyle(color)
                                .symbolSize(14)
                        }
                    }
                }
            }
            .modifier(SparkScale(days: values.count, bars: bars))
            .chartYScale(domain: 0...top)
            .chartXAxis(.hidden)
            .chartYAxis(.hidden)
            .chartLegend(.hidden)
        }
    }
}

/// Bars take every day as a category; lines a number line over the days.
private struct SparkScale: ViewModifier {
    let days: Int
    let bars: Bool

    @ViewBuilder
    func body(content: Content) -> some View {
        if bars {
            content.chartXScale(domain: (0..<max(days, 1)).map { String($0) })
        } else {
            content.chartXScale(domain: -0.5...(Double(max(days, 1)) - 0.5))
        }
    }
}

// MARK: - gauges

private struct GaugeGrid: View {
    let gauges: [ControlGauge]

    var body: some View {
        LazyVGrid(columns: [GridItem(.flexible(), spacing: 10), GridItem(.flexible(), spacing: 10)],
                  spacing: 10) {
            ForEach(gauges, id: \.title) { g in
                GaugeCard(gauge: g)
            }
        }
    }
}

/// A hooks metric: the value, its baseline, the delta in its tone, a ring
/// (or, for a reset against its threshold, a bar with the threshold
/// ticked), and its last 14 days.
private struct GaugeCard: View {
    let gauge: ControlGauge
    @Environment(\.theme) private var theme

    var body: some View {
        let g = gauge
        let color = theme.delta(g.tone)
        VStack(alignment: .leading, spacing: 6) {
            Text(g.title.uppercased())
                .font(.tiny)
                .foregroundStyle(theme.muted)
                .lineLimit(1)
            HStack(alignment: .top, spacing: 6) {
                VStack(alignment: .leading, spacing: 4) {
                    HStack(spacing: 4) {
                        Text(g.valueLabel)
                            .font(.heading)
                            .foregroundStyle(theme.bright)
                            .lineLimit(1)
                            .minimumScaleFactor(0.7)
                        if g.threshold && g.tone == .flag {
                            Text("⚠").foregroundStyle(theme.warn)
                        }
                    }
                    if let b = g.baselineLabel {
                        Text("vs \(b)")
                            .font(.small)
                            .foregroundStyle(theme.muted)
                            .lineLimit(1)
                    }
                    HStack(spacing: 6) {
                        if let d = g.delta {
                            Text(d)
                                .font(.small)
                                .foregroundStyle(g.tone == .plain ? theme.text : color)
                                .padding(.horizontal, 8)
                                .padding(.vertical, 2)
                                .overlay(Capsule().stroke(color, lineWidth: 1.2))
                        }
                        if let a = g.aside {
                            Text(a).font(.tiny).foregroundStyle(theme.faint)
                        }
                    }
                }
                Spacer(minLength: 0)
                if !g.threshold {
                    Ring(gauge: g, color: color)
                        .frame(width: 46, height: 46)
                }
            }
            if g.threshold {
                Meter(gauge: g, color: color)
                    .frame(height: 8)
            }
            Spark(values: g.spark, color: color)
                .frame(height: 30)
        }
        .modifier(Card())
        .accessibilityElement(children: .combine)
    }
}

/// How full a gauge draws: a percent of 100, else the value on a scale a
/// quarter past the larger of value and baseline, so both stay inside.
private func gaugeFill(_ g: ControlGauge) -> (value: Double, baseline: Double?) {
    let scale = g.unit == .pct ? 100 : max(g.value, g.baseline ?? 0) * 1.25
    guard scale > 0 else { return (0, nil) }
    return (min(max(g.value / scale, 0), 1), g.baseline.map { min(max($0 / scale, 0), 1) })
}

private struct Ring: View {
    let gauge: ControlGauge
    let color: Color
    @Environment(\.theme) private var theme

    var body: some View {
        let f = gaugeFill(gauge)
        ZStack {
            Circle().stroke(theme.idle, lineWidth: 7)
            Circle()
                .trim(from: 0, to: f.value)
                .stroke(color, style: StrokeStyle(lineWidth: 7, lineCap: .round))
                .rotationEffect(.degrees(-90))
            if let b = f.baseline {
                // The baseline as a tick across the ring.
                Capsule()
                    .fill(theme.bright)
                    .frame(width: 2, height: 11)
                    .offset(y: -23)
                    .rotationEffect(.degrees(360 * b))
            }
        }
    }
}

private struct Meter: View {
    let gauge: ControlGauge
    let color: Color
    @Environment(\.theme) private var theme

    var body: some View {
        let f = gaugeFill(gauge)
        GeometryReader { geo in
            ZStack(alignment: .leading) {
                Capsule().fill(theme.idle)
                Capsule().fill(color).frame(width: geo.size.width * f.value)
                if let b = f.baseline {
                    Rectangle()
                        .fill(theme.bright)
                        .frame(width: 2, height: geo.size.height + 8)
                        .offset(x: geo.size.width * b - 1)
                }
            }
        }
    }
}

/// The newest hooks note's seats, a dot each, and how many are alive.
private struct SeatsLine: View {
    let seats: [ControlSeat]
    @Environment(\.theme) private var theme

    var body: some View {
        let alive = seats.filter { $0.health == .ok }.count
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 10) {
                Text("seats").foregroundStyle(theme.muted)
                ForEach(seats, id: \.name) { s in
                    HStack(spacing: 5) {
                        Circle().fill(theme.health(s.health)).frame(width: 8, height: 8)
                        Text(s.name).foregroundStyle(theme.text)
                    }
                }
                Text("\(alive)/\(seats.count) alive").foregroundStyle(theme.faint)
            }
            .font(.small)
        }
        .modifier(Card())
    }
}

// MARK: - drill-down

/// The bar on top of a pushed page: back, and where we are.
private struct BackBar: View {
    let title: String
    let back: () -> Void
    @Environment(\.theme) private var theme

    var body: some View {
        HStack(spacing: 8) {
            Button(action: back) {
                Text("‹ control")
                    .font(.chip)
                    .foregroundStyle(theme.link)
                    .frame(minHeight: 44)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            Text(title)
                .font(.chipOn)
                .foregroundStyle(theme.bright)
                .lineLimit(1)
            Spacer()
        }
        .padding(.horizontal, 16)
    }
}

/// A topic's notes, newest day first: day, source, status, one-liner.
private struct TopicDays: View {
    let client: Client
    let topic: String
    let back: () -> Void
    let pick: (String) -> Void
    @Environment(Model.self) private var model
    @Environment(\.theme) private var theme

    var body: some View {
        let _ = model.version
        let rows = client.controlDays(topic: topic)
        VStack(spacing: 0) {
            BackBar(title: topic, back: back)
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 8) {
                    if rows.isEmpty {
                        Text("no notes for this topic yet")
                            .font(.small)
                            .foregroundStyle(theme.muted)
                            .frame(maxWidth: .infinity)
                            .padding(.top, 40)
                    }
                    ForEach(rows, id: \.id) { r in
                        row(r)
                            .contentShape(Rectangle())
                            .onTapGesture { pick(r.id) }
                    }
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 8)
            }
        }
    }

    private func row(_ r: ControlDay) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                Text(r.day).font(.textBold).foregroundStyle(theme.bright)
                Text(r.source).font(.small).foregroundStyle(theme.muted).lineLimit(1)
                Spacer(minLength: 4)
                if let s = r.status {
                    StatusChip(text: s, color: theme.status(s))
                }
                Text("›").foregroundStyle(theme.faint)
            }
            Text(r.line)
                .font(.small)
                .foregroundStyle(theme.text)
                .lineLimit(2)
        }
        .modifier(Card())
    }
}

private struct StatusChip: View {
    let text: String
    let color: Color
    @Environment(\.theme) private var theme

    var body: some View {
        Text(text)
            .font(.tiny)
            .foregroundStyle(theme.onAccent)
            .padding(.horizontal, 7)
            .padding(.vertical, 2)
            .background(color, in: Capsule())
    }
}

/// One note: who and when, its summary, its reactions, its request with
/// the vote buttons; for hooks/1 the status strip, gauges, trend charts
/// and tables; for anything else the details as JSON.
private struct NotePage: View {
    let client: Client
    let id: String
    let open: (String) -> Void
    let back: () -> Void
    @Environment(Model.self) private var model
    @Environment(\.theme) private var theme

    var body: some View {
        let _ = model.version
        let n = client.controlNote(id: id)
        VStack(spacing: 0) {
            BackBar(title: n.map { "\($0.topic) · \($0.day ?? "no day")" } ?? "note", back: back)
            ScrollView {
                if let n {
                    content(n)
                        .padding(.horizontal, 16)
                        .padding(.top, 8)
                        .padding(.bottom, 40)
                } else {
                    Text("this note isn't loaded")
                        .font(.small)
                        .foregroundStyle(theme.muted)
                        .padding(.top, 40)
                }
            }
        }
    }

    private func content(_ n: ControlNote) -> some View {
        VStack(alignment: .leading, spacing: 16) {
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 8) {
                    Circle().fill(theme.health(n.health)).frame(width: 9, height: 9)
                    Text(n.source ?? "no source").font(.textBold).foregroundStyle(theme.bright)
                    Text(n.schema ?? "no schema").font(.small).foregroundStyle(theme.muted)
                }
                Text(byline(n))
                    .font(.small)
                    .foregroundStyle(theme.faint)
            }
            Text(rich(n.summary, mentions: [], theme: theme))
                .font(.text)
                .foregroundStyle(theme.text)
                .textSelection(.enabled)
            if !n.reactions.isEmpty {
                HStack(spacing: 6) {
                    ForEach(n.reactions, id: \.emoji) { r in
                        Text("\(r.emoji) \(r.count)")
                            .font(.small)
                            .foregroundStyle(theme.text)
                            .padding(.horizontal, 8)
                            .padding(.vertical, 3)
                            .background(theme.chipOff, in: Capsule())
                    }
                }
            }
            if let r = n.request {
                RequestBlock(client: client, request: r)
                    .modifier(Card(stroke: r.status == "open" ? theme.warn : nil))
            }
            if !n.strip.isEmpty {
                StatusStrip(days: n.strip)
            }
            if !n.gauges.isEmpty {
                GaugeGrid(gauges: n.gauges)
            }
            ForEach(n.charts, id: \.title) { c in
                TrendChart(chart: c, days: n.chartDays)
            }
            ForEach(n.tables, id: \.title) { t in
                TableView(table: t)
            }
            if let json = n.detailsJson {
                Text("details (\(n.schema ?? "no schema"), as JSON)")
                    .font(.small)
                    .foregroundStyle(theme.muted)
                CodeBlock(lang: "json", code: json)
            }
            if let e = n.detailsError {
                Text("details unreadable: \(e)")
                    .font(.small)
                    .foregroundStyle(theme.page)
            }
            ForEach(n.files, id: \.self) { f in
                Text("▣ \(f)").font(.small).foregroundStyle(theme.muted)
            }
            Button("open in #control") { open(n.link) }
                .font(.chip)
                .foregroundStyle(theme.link)
        }
    }

    private func byline(_ n: ControlNote) -> String {
        var parts = ["by \(n.author)", stamp(n.at)]
        if n.replies > 0 {
            parts.append("\(n.replies) \(n.replies == 1 ? "reply" : "replies")")
        }
        return parts.joined(separator: " · ")
    }
}

/// Each day's hooks status as a cell, oldest first; a gap is an empty cell.
private struct StatusStrip: View {
    let days: [ControlStatusDay]
    @Environment(\.theme) private var theme

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("hooks by day").font(.tiny).foregroundStyle(theme.muted)
            HStack(spacing: 3) {
                ForEach(days, id: \.day) { d in
                    RoundedRectangle(cornerRadius: 3)
                        .fill(d.status == nil ? theme.idle : theme.health(d.health))
                        .frame(height: 16)
                        .accessibilityLabel("\(d.day) \(d.status ?? "no note")")
                }
            }
            HStack {
                Text(days.first.map { String($0.day.dropFirst(5)) } ?? "")
                Spacer()
                Text(days.last.map { String($0.day.dropFirst(5)) } ?? "")
            }
            .font(.tiny)
            .foregroundStyle(theme.faint)
        }
    }
}

/// A trend over the note's days: each series as a line (broken at gaps),
/// its baseline as a dashed rule, the newest values in the legend.
private struct TrendChart: View {
    let chart: ControlChart
    let days: [String]
    @Environment(\.theme) private var theme

    /// Series colors: the data cyan first, then violet, green, amber.
    private func color(_ i: Int) -> Color {
        let hues: [Float] = [215, 300, 145, 75]
        return theme.hue(hues[i % hues.count])
    }

    private struct Point: Identifiable {
        let id: String
        let day: Int
        let value: Double
        let run: String
    }

    private func points(_ s: ControlSeries, _ i: Int) -> [Point] {
        var out: [Point] = []
        var run = 0
        var last = -2
        for (d, v) in s.points.enumerated() {
            guard let v else { continue }
            if d != last + 1 { run += 1 }
            last = d
            out.append(Point(id: "\(i)-\(d)", day: d, value: v, run: "\(i)-\(run)"))
        }
        return out
    }

    var body: some View {
        let unit = chart.unit
        VStack(alignment: .leading, spacing: 8) {
            Text(chart.title).font(.chipOn).foregroundStyle(theme.bright)
            HStack(spacing: 12) {
                ForEach(Array(chart.series.enumerated()), id: \.offset) { i, s in
                    HStack(spacing: 4) {
                        Circle().fill(color(i)).frame(width: 7, height: 7)
                        Text(legend(s, unit)).font(.tiny).foregroundStyle(theme.muted).lineLimit(1)
                    }
                }
            }
            Chart {
                ForEach(Array(chart.series.enumerated()), id: \.offset) { i, s in
                    let pts = points(s, i)
                    ForEach(pts) { p in
                        LineMark(x: .value("day", Double(p.day)), y: .value("value", p.value),
                                 series: .value("run", p.run))
                            .foregroundStyle(color(i))
                            .lineStyle(StrokeStyle(lineWidth: 2, lineCap: .round, lineJoin: .round))
                        PointMark(x: .value("day", Double(p.day)), y: .value("value", p.value))
                            .foregroundStyle(color(i))
                            .symbolSize(pts.count == 1 ? 30 : 12)
                    }
                    if let b = s.baseline {
                        RuleMark(y: .value("baseline", b))
                            .foregroundStyle(color(i).opacity(0.55))
                            .lineStyle(StrokeStyle(lineWidth: 1, dash: [4, 3]))
                    }
                }
            }
            .chartXScale(domain: -0.3...(Double(max(days.count - 1, 0)) + 0.3))
            .chartXAxis {
                AxisMarks(values: ticks) { v in
                    AxisGridLine().foregroundStyle(theme.rule)
                    AxisValueLabel {
                        if let d = v.as(Double.self), days.indices.contains(Int(d.rounded())) {
                            let i = Int(d.rounded())
                            Text(String(days[i].dropFirst(5))).font(.tiny).foregroundStyle(theme.faint)
                        }
                    }
                }
            }
            .chartYAxis {
                AxisMarks(position: .leading, values: .automatic(desiredCount: 4)) { v in
                    AxisGridLine().foregroundStyle(theme.rule)
                    AxisValueLabel {
                        if let y = v.as(Double.self) {
                            Text(controlValueLabel(value: y, unit: unit)).font(.tiny).foregroundStyle(theme.faint)
                        }
                    }
                }
            }
            .frame(height: 150)
        }
        .modifier(Card())
    }

    /// At most five day labels, first and last included.
    private var ticks: [Double] {
        let n = days.count
        guard n > 1 else { return n == 1 ? [0] : [] }
        let step = max(1, Int((Double(n - 1) / 4).rounded(.up)))
        var t = Array(stride(from: 0, to: n - 1, by: step))
        t.append(n - 1)
        return t.map(Double.init)
    }

    private func legend(_ s: ControlSeries, _ unit: ControlUnit) -> String {
        var out = s.label
        if let v = s.points.last(where: { $0 != nil }) ?? nil {
            out += " \(controlValueLabel(value: v, unit: unit))"
        }
        if let b = s.baseline {
            out += " vs \(controlValueLabel(value: b, unit: unit))"
        }
        return out
    }
}

/// A table that scrolls sideways; hot rows take their status color.
private struct TableView: View {
    let table: ControlTable
    @Environment(\.theme) private var theme

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(table.title).font(.chipOn).foregroundStyle(theme.bright)
            ScrollView(.horizontal, showsIndicators: false) {
                Grid(alignment: .leading, horizontalSpacing: 14, verticalSpacing: 6) {
                    GridRow {
                        ForEach(Array(table.head.enumerated()), id: \.offset) { _, h in
                            Text(h).foregroundStyle(theme.faint)
                        }
                    }
                    ForEach(Array(table.rows.enumerated()), id: \.offset) { _, r in
                        GridRow {
                            ForEach(Array(r.cells.enumerated()), id: \.offset) { _, c in
                                Text(c).foregroundStyle(r.hot.map { theme.health($0) } ?? theme.text)
                            }
                        }
                    }
                }
                .font(.small)
                .fixedSize()
            }
        }
        .modifier(Card())
    }
}
