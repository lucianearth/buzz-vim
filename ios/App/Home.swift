import OSLog
import SwiftUI

private let holdLog = Logger(subsystem: "com.lucianhymer.buzzvim", category: "hold")

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
/// hold a tab for the switcher (every tab in tall rows, drag handles to
/// reorder, a Sections button), hold the status line for sections.
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
    @State private var switcher = false

    var body: some View {
        let _ = model.version
        let tabs = client.tabs()
        NavigationStack(path: $path) {
            VStack(spacing: 0) {
                Header(client: client, tabs: tabs, selected: $selected,
                       showAgents: $showAgents, showSections: $showSections, switcher: $switcher)
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
            if switcher {
                TabSwitcher(client: client, tabs: tabs, selected: $selected,
                            shown: $switcher, showSections: $showSections)
                    .transition(.opacity)
            }
        }
        .sensoryFeedback(.impact, trigger: switcher) { _, now in now }
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
    @Binding var switcher: Bool
    @Environment(Model.self) private var model
    @Environment(\.theme) private var theme
    /// Each tab's place in the row, to tell which one a hold landed on.
    @State private var chipFrames: [String: CGRect] = [:]

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
                                .onGeometryChange(for: CGRect.self) { $0.frame(in: .named("tabrow")) } action: {
                                    chipFrames[t.id] = $0
                                }
                        }
                    }
                    .padding(.horizontal, 22)
                    .coordinateSpace(.named("tabrow"))
                    // Hold a tab: the switcher opens. One UIKit long press for
                    // the row; it fails if the finger moves early, so the
                    // scroll view keeps swipes.
                    .gesture(Hold(began: pickUp))
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
            dots
        }
        .padding(.top, 16)
        .padding(.bottom, 16)
        .contentShape(Rectangle())
        // Simultaneous: as `.gesture` it took sideways swipes from the tab row.
        .simultaneousGesture(DragGesture(minimumDistance: 24).onEnded { g in
            if !switcher,
               g.translation.height > 50 && abs(g.translation.width) < g.translation.height {
                showAgents = true
            }
        })
    }

    private func pickUp(_ at: CGPoint) {
        guard tabs.contains(where: { chipFrames[$0.id]?.contains(at) == true }) else {
            holdLog.info("hold missed at \(at.x), \(at.y); \(chipFrames.count) frames")
            return
        }
        withAnimation(.easeOut(duration: 0.15)) { switcher = true }
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

/// Hold 0.3 s; `began` gets the point in the view the hold is on.
/// UIKit's long press fails as soon as the finger moves early, which hands
/// the touch to the scroll view; a SwiftUI long press kept it.
struct Hold: UIGestureRecognizerRepresentable {
    let began: (CGPoint) -> Void

    func makeUIGestureRecognizer(context: Context) -> UILongPressGestureRecognizer {
        let g = UILongPressGestureRecognizer()
        g.minimumPressDuration = 0.3
        return g
    }

    func handleUIGestureRecognizerAction(_ g: UILongPressGestureRecognizer, context: Context) {
        if g.state == .began { began(context.converter.location(in: .local)) }
    }
}

/// Every tab in tall rows, by section: tap one to go there, drag a handle
/// to reorder it within its section. It flips down from the tab row and
/// stays until a tab, Sections or Close.
struct TabSwitcher: View {
    let client: Client
    let tabs: [ChannelTab]
    @Binding var selected: String?
    @Binding var shown: Bool
    @Binding var showSections: Bool
    @Environment(Model.self) private var model
    @Environment(\.theme) private var theme
    @State private var open = false

    var body: some View {
        let names = Dictionary(client.sections().map { ($0.id, $0.name) }, uniquingKeysWith: { a, _ in a })
        VStack(spacing: 0) {
            ScrollViewReader { proxy in
                List {
                    ForEach(groups, id: \.0) { section, items in
                        Section {
                            ForEach(items, id: \.id) { t in
                                Button { pick(t.id) } label: { row(t) }
                                    .buttonStyle(.plain)
                                    .id(t.id)
                                    .listRowBackground(theme.canvas)
                                    .listRowSeparator(.hidden)
                                    .listRowInsets(EdgeInsets(top: 4, leading: 22, bottom: 4, trailing: 12))
                            }
                            .onMove { from, to in
                                var ids = items.map(\.id)
                                ids.move(fromOffsets: from, toOffset: to)
                                model.order(section, ids)
                            }
                        } header: {
                            Text(names[section] ?? section)
                                .font(.small)
                                .foregroundStyle(theme.faint)
                        }
                    }
                }
                .listStyle(.plain)
                .scrollContentBackground(.hidden)
                // Edit mode for the drag handles; there's no delete.
                .environment(\.editMode, .constant(.active))
                .onAppear { proxy.scrollTo(selected, anchor: .center) }
            }
            HStack {
                Button("Sections") {
                    close()
                    showSections = true
                }
                Spacer()
                Button("Close") { close() }
            }
            .font(.label)
            .foregroundStyle(theme.accent)
            .padding(.horizontal, 22)
            .padding(.vertical, 14)
        }
        .background(theme.canvas.ignoresSafeArea())
        // Flip down from the tab row.
        .rotation3DEffect(.degrees(open ? 0 : -70), axis: (1, 0, 0), anchor: .top, perspective: 0.6)
        .opacity(open ? 1 : 0)
        .onAppear { withAnimation(.spring(duration: 0.35)) { open = true } }
    }

    /// Sections in tab order, each with its tabs.
    private var groups: [(String, [ChannelTab])] {
        var out: [(String, [ChannelTab])] = []
        for t in tabs {
            if let i = out.firstIndex(where: { $0.0 == t.section }) {
                out[i].1.append(t)
            } else {
                out.append((t.section, [t]))
            }
        }
        return out
    }

    private func pick(_ id: String) {
        selected = id
        close()
    }

    private func close() {
        withAnimation(.easeOut(duration: 0.2)) { shown = false }
    }

    private func row(_ t: ChannelTab) -> some View {
        let on = t.id == selected
        let name = t.label.hasPrefix("#") ? String(t.label.dropFirst()) : t.label
        return HStack(spacing: 8) {
            Text(name)
                .font(on ? .chipOn : .chip)
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
        .foregroundStyle(on ? theme.onAccent : theme.muted)
        .padding(.horizontal, 16)
        .frame(maxWidth: .infinity, minHeight: 52, alignment: .leading)
        .background(on ? theme.chipOn : theme.chipOff, in: RoundedRectangle(cornerRadius: 10))
        .contentShape(Rectangle())
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
