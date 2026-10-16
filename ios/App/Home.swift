import SwiftUI

struct Route: Hashable {
    let channel: String
    let root: String
    /// A reply to open the thread on, at the top (the jump key).
    var focus: String? = nil
}

/// Tab moves for the keyboard bar, from wherever the compose box is.
struct Nav {
    var step: (Int) -> Void = { _ in }
    var nextUnread: () -> Void = {}
}

private struct NavKey: EnvironmentKey {
    static let defaultValue = Nav()
}

extension EnvironmentValues {
    var nav: Nav {
        get { self[NavKey.self] }
        set { self[NavKey.self] = newValue }
    }
}

/// Tabs across the top, one channel per page: swipe sideways between them,
/// tap a message for its thread, pull the header down for the agents,
/// hold a tab to drag it (the row unfolds into a column), hold the status
/// line for sections.
struct Home: View {
    let client: Client
    @Environment(Model.self) private var model
    @Environment(\.theme) private var theme
    @State private var path: [Route] = []
    @State private var selected: String? = UserDefaults.standard.string(forKey: "active")
    /// The channel the user last chose. Survives a fallback pick while the
    /// tab list is still filling in, so a relaunch lands back on it.
    @State private var wanted: String? = UserDefaults.standard.string(forKey: "active")
    @State private var fallback = false
    @State private var showAgents = false
    @State private var showSections = false
    @State private var lift: Lift?

    var body: some View {
        let _ = model.version
        let tabs = client.tabs()
        NavigationStack(path: $path) {
            VStack(spacing: 0) {
                Header(client: client, tabs: tabs, selected: $selected,
                       showAgents: $showAgents, showSections: $showSections, lift: $lift)
                if tabs.isEmpty {
                    Spacer()
                    Text(client.ready() ? "no channels yet" : "loading channels…")
                        .font(.small)
                        .foregroundStyle(theme.muted)
                    Spacer()
                } else {
                    // A paging scroll view, not a paged TabView: the TabView
                    // stalls halfway between pages when the selection moves
                    // while a compose field holds the keyboard.
                    ScrollView(.horizontal) {
                        LazyHStack(spacing: 0) {
                            ForEach(tabs, id: \.id) { t in
                                ChannelView(client: client, channel: t.id) { root in
                                    path.append(Route(channel: t.id, root: root))
                                }
                                .containerRelativeFrame([.horizontal, .vertical])
                                .id(t.id)
                            }
                        }
                        .scrollTargetLayout()
                    }
                    .scrollIndicators(.hidden)
                    .scrollTargetBehavior(.paging)
                    .scrollPosition(id: $selected)
                    // Rebuild when the tab list changes, so a restored
                    // selection lands on its page.
                    .id(tabs.map(\.id))
                }
            }
            .background(theme.canvas)
            .toolbar(.hidden, for: .navigationBar)
            .navigationDestination(for: Route.self) { r in
                ThreadView(client: client, route: r)
            }
        }
        .overlay {
            if lift != nil {
                TabColumn(lift: $lift, tabs: tabs)
                    .transition(.opacity)
            }
        }
        .sensoryFeedback(.impact, trigger: lift?.held)
        .environment(\.nav, Nav(step: step, nextUnread: nextUnread))
        .onAppear { settle(tabs) }
        .onChange(of: tabs.map(\.id)) { _, _ in settle(client.tabs()) }
        .onChange(of: selected) { _, now in
            client.openChannel(channel: now)
            if fallback {
                fallback = false
            } else {
                wanted = now
                UserDefaults.standard.set(now, forKey: "active")
            }
        }
        .sheet(isPresented: $showAgents) {
            AgentsView(client: client)
                .presentationDetents([.medium, .large])
                .presentationDragIndicator(.visible)
                .modifier(Themed())
        }
        .sheet(isPresented: $showSections) {
            SectionsView(client: client)
                .presentationDetents([.medium, .large])
                .presentationDragIndicator(.visible)
                .modifier(Themed())
        }
    }

    /// The tab `by` places along, wrapping. From a thread, back to the
    /// channel first. The keyboard stays up on the new tab.
    private func step(_ by: Int) {
        let tabs = client.tabs()
        guard !tabs.isEmpty else { return }
        let i = tabs.firstIndex { $0.id == selected } ?? 0
        let id = tabs[((i + by) % tabs.count + tabs.count) % tabs.count].id
        model.focusKey = id
        go(id)
    }

    /// The bell key: the first unread thing in this tab, oldest first (a
    /// reply opens its thread on it). Nothing here: on to the next tab
    /// with something unread, mentions first.
    private func nextUnread() {
        if let s = selected, let j = client.nextUnread(channel: s) {
            land(s, j)
            return
        }
        let tabs = client.tabs()
        guard !tabs.isEmpty else { return }
        let i = tabs.firstIndex { $0.id == selected } ?? -1
        let after = (1...tabs.count).map { tabs[(i + $0 + tabs.count) % tabs.count] }
            .filter { $0.id != selected }
        guard let t = after.first(where: { $0.mentions > 0 }) ?? after.first(where: { $0.unread > 0 }) else {
            model.say("nothing unread")
            return
        }
        // Open it now, so Rust notes where it was read up to before the jump.
        client.openChannel(channel: t.id)
        go(t.id)
        if let j = client.nextUnread(channel: t.id) {
            land(t.id, j)
        } else {
            model.focusKey = t.id
        }
    }

    private func land(_ channel: String, _ j: Jump) {
        if let root = j.root {
            model.focusKey = "\(channel)/\(root)"
            path = [Route(channel: channel, root: root, focus: j.id)]
        } else {
            model.focusKey = channel
            path = []
            model.scrollTarget = (channel, j.id)
        }
    }

    /// Key-row moves only, so the keyboard is up: jump, no page slide.
    private func go(_ id: String) {
        path = []
        selected = id
    }

    /// Keep the selection on a tab that exists, and tell Rust what's open.
    private func settle(_ tabs: [ChannelTab]) {
        if let w = wanted, w != selected, tabs.contains(where: { $0.id == w }) {
            selected = w
        } else if let s = selected, tabs.contains(where: { $0.id == s }) {
            client.openChannel(channel: s)
        } else if let first = tabs.first?.id, first != selected {
            fallback = true
            selected = first
        }
    }
}

struct Header: View {
    let client: Client
    let tabs: [ChannelTab]
    @Binding var selected: String?
    @Binding var showAgents: Bool
    @Binding var showSections: Bool
    @Binding var lift: Lift?
    @Environment(Model.self) private var model
    @Environment(\.theme) private var theme
    /// False again if the system cancels the hold, so the column can't stick.
    @GestureState private var holding = false

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            status
                .padding(.horizontal, 22)
                .onTapGesture { showAgents = true }
                .onLongPressGesture { showSections = true }
            ScrollViewReader { proxy in
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 8) {
                        ForEach(tabs, id: \.id) { t in
                            chip(t)
                                .id(t.id)
                                .onTapGesture { withAnimation(.easeOut(duration: 0.2)) { selected = t.id } }
                                // Hold a tab: its section unfolds into a column
                                // to slide it up or down in.
                                .gesture(hold(t))
                        }
                    }
                    .padding(.horizontal, 22)
                }
                .onChange(of: selected) { _, now in
                    withAnimation { proxy.scrollTo(now, anchor: .center) }
                }
                // On launch the restored tab may be off to the right.
                .onAppear { proxy.scrollTo(selected, anchor: .center) }
                .onChange(of: tabs.count) { _, _ in
                    DispatchQueue.main.async { proxy.scrollTo(selected, anchor: .center) }
                }
            }
            .onChange(of: holding) { _, now in if !now { drop() } }
            dots
        }
        .padding(.top, 16)
        .padding(.bottom, 16)
        .contentShape(Rectangle())
        .gesture(DragGesture(minimumDistance: 24).onEnded { g in
            if g.translation.height > 50 && abs(g.translation.width) < g.translation.height {
                showAgents = true
            }
        })
    }

    private func hold(_ t: ChannelTab) -> some Gesture {
        LongPressGesture(minimumDuration: 0.3)
            .sequenced(before: DragGesture(minimumDistance: 0, coordinateSpace: .global))
            .updating($holding) { v, state, _ in
                if case .second(true, _) = v { state = true }
            }
            .onChanged { v in
                guard case .second(true, let drag) = v else { return }
                if lift == nil {
                    let ids = tabs.filter { $0.section == t.section }.map(\.id)
                    withAnimation(.easeOut(duration: 0.15)) {
                        lift = Lift(section: t.section, ids: ids, held: t.id)
                    }
                }
                lift?.dy = drag?.translation.height ?? 0
            }
            .onEnded { _ in drop() }
    }

    /// Let go: save the new order and fold the column away.
    private func drop() {
        guard let l = lift else { return }
        if l.order != l.ids { model.order(l.section, l.order) }
        withAnimation(.easeOut(duration: 0.2)) { lift = nil }
    }

    private var status: some View {
        let working = client.workingAnywhere()
        let conn = client.conn()
        let text: String
        let dot: Color
        if !working.isEmpty {
            text = working.joined(separator: " · ") + " working"
            dot = theme.live
        } else {
            switch conn {
            case .online: text = "online"; dot = theme.live
            case .connecting: text = "connecting…"; dot = theme.faint
            case .offline: text = "offline, retrying"; dot = theme.badge
            }
        }
        return HStack(spacing: 7) {
            Pulse(color: dot, on: !working.isEmpty)
            Text(text)
                .font(.text)
                .foregroundStyle(theme.muted)
                .lineLimit(1)
        }
    }

    /// Where the open tab sits in the row, like a page indicator.
    private var dots: some View {
        HStack(spacing: 8) {
            ForEach(tabs, id: \.id) { t in
                Circle()
                    .fill(t.id == selected ? theme.chipOn : theme.rule)
                    .frame(width: 7, height: 7)
            }
        }
        .frame(maxWidth: .infinity)
    }

    private func chip(_ t: ChannelTab) -> some View {
        let on = t.id == selected
        let name = t.label.hasPrefix("#") ? String(t.label.dropFirst()) : t.label
        return HStack(spacing: 5) {
            Text(name)
                .font(on ? .chipOn : .chip)
                .lineLimit(1)
            if t.mentions > 0 && !on {
                Text("\(t.mentions)")
                    .font(.tiny)
                    .foregroundStyle(theme.onAccent)
                    .padding(.horizontal, 5)
                    .frame(minWidth: 17, minHeight: 17)
                    .background(theme.badge, in: Capsule())
            } else if t.unread > 0 && !on {
                Circle().fill(theme.badge).frame(width: 6, height: 6)
            }
        }
        .foregroundStyle(on ? theme.onAccent : theme.muted)
        .padding(.horizontal, 16)
        .padding(.vertical, 11)
        .background(on ? theme.chipOn : theme.chipOff, in: RoundedRectangle(cornerRadius: 10))
    }
}

/// A held tab and its section's tabs, while it's being dragged.
struct Lift {
    let section: String
    /// The section's tabs as they were when the hold started.
    let ids: [String]
    let held: String
    /// How far the finger has moved since, and one row's height.
    var dy: CGFloat = 0
    var rowH: CGFloat = 56

    var start: Int { ids.firstIndex(of: held) ?? 0 }
    var target: Int { min(max(start + Int((dy / rowH).rounded()), 0), ids.count - 1) }
    var order: [String] {
        var o = ids.filter { $0 != held }
        o.insert(held, at: target)
        return o
    }
}

/// The section the held tab is in, unfolded into tall rows. The held one
/// follows the finger; the rest make room as it passes.
struct TabColumn: View {
    @Binding var lift: Lift?
    let tabs: [ChannelTab]
    @Environment(\.theme) private var theme
    @State private var open = false

    var body: some View {
        GeometryReader { geo in
            if let l = lift {
                let byId = Dictionary(tabs.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })
                ZStack(alignment: .top) {
                    theme.canvas.opacity(0.97).ignoresSafeArea()
                    ForEach(Array(l.order.enumerated()), id: \.element) { i, id in
                        if let t = byId[id] {
                            let held = id == l.held
                            let slot = held
                                ? min(max(CGFloat(l.start) * l.rowH + l.dy, 0), CGFloat(l.ids.count - 1) * l.rowH)
                                : CGFloat(i) * l.rowH
                            row(t, held: held)
                                .frame(height: l.rowH - 8)
                                .padding(.horizontal, 22)
                                // Unfold: each row drops down from the tab row
                                // and flips open, one just after another.
                                .rotation3DEffect(.degrees(open ? 0 : -80), axis: (1, 0, 0),
                                                  anchor: .top, perspective: 0.6)
                                .offset(y: 16 + (open ? slot : 0))
                                .opacity(open ? 1 : 0)
                                .animation(.spring(duration: 0.35).delay(Double(i) * 0.02), value: open)
                                .animation(held ? nil : .snappy(duration: 0.2), value: l.target)
                                .zIndex(held ? 1 : 0)
                        }
                    }
                }
                .onAppear {
                    lift?.rowH = min(64, max(36, (geo.size.height - 32) / CGFloat(max(l.ids.count, 1))))
                    open = true
                }
                .sensoryFeedback(.selection, trigger: l.target)
            }
        }
    }

    private func row(_ t: ChannelTab, held: Bool) -> some View {
        let name = t.label.hasPrefix("#") ? String(t.label.dropFirst()) : t.label
        return HStack(spacing: 8) {
            Text(name)
                .font(held ? .chipOn : .chip)
                .lineLimit(1)
            Spacer()
            if t.mentions > 0 {
                Text("\(t.mentions)")
                    .font(.tiny)
                    .foregroundStyle(theme.onAccent)
                    .padding(.horizontal, 5)
                    .frame(minWidth: 17, minHeight: 17)
                    .background(theme.badge, in: Capsule())
            } else if t.unread > 0 {
                Circle().fill(theme.badge).frame(width: 6, height: 6)
            }
        }
        .foregroundStyle(held ? theme.onAccent : theme.muted)
        .padding(.horizontal, 16)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
        .background(held ? theme.chipOn : theme.chipOff, in: RoundedRectangle(cornerRadius: 10))
        .scaleEffect(held ? 1.03 : 1)
        .shadow(color: .black.opacity(held ? 0.35 : 0), radius: 10, y: 4)
    }
}

/// A small dot that breathes while something is working.
struct Pulse: View {
    let color: Color
    let on: Bool
    @State private var big = false

    var body: some View {
        Circle()
            .fill(color)
            .frame(width: 7, height: 7)
            .scaleEffect(on && big ? 1.35 : 1)
            .opacity(on && big ? 0.6 : 1)
            .animation(on ? .easeInOut(duration: 0.9).repeatForever(autoreverses: true) : .default, value: big)
            .onAppear { big = true }
    }
}
