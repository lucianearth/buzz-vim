import OSLog
import SwiftUI

private let holdLog = Logger(subsystem: "com.lucianhymer.buzzvim", category: "hold")

struct Route: Hashable {
    let channel: String
    let root: String
    /// A reply to open the thread on, at the top (the bell).
    var focus: String? = nil
}

/// Tab moves for the keyboard bar, from wherever the compose box is.
struct Nav {
    var step: (Int) -> Void = { _ in }
    var bell: () -> Void = {}
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

/// Tabs across the top, Needs you (tab 0) first, then one channel per
/// page: swipe sideways between them,
/// tap a message for its thread (it opens in the tab, which remembers it;
/// swipes still change tabs), pull the header down for the agents,
/// hold a tab or the status line for the switcher
/// (a page of tall rows per section, drag handles to reorder).
struct Home: View {
    let client: Client
    @Environment(Model.self) private var model
    @Environment(\.theme) private var theme
    /// The thread open in each tab, by channel; kept across launches.
    @State private var threads: [String: Route] = Home.savedThreads()
    @State private var selected: String? = UserDefaults.standard.string(forKey: "active")
    /// The channel the user last chose. Survives a fallback pick while the
    /// tab list is still filling in, so a relaunch lands back on it.
    @State private var wanted: String? = UserDefaults.standard.string(forKey: "active")
    @State private var fallback = false
    @State private var showAgents = false
    @State private var showChecklist = false
    @State private var switcher = false
    /// The Q sheet, while it's up.
    @State private var quick: QuickBox?

    var body: some View {
        let _ = model.version
        let tabs = client.tabs()
        VStack(spacing: 0) {
            Header(client: client, tabs: tabs, selected: $selected,
                   showAgents: $showAgents, showChecklist: $showChecklist,
                   switcher: $switcher, openSwitcher: openSwitcher, openQuick: openQuick)
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
                        // Tab 0 first, always.
                        NeedsView(client: client) { link in
                            if let f = client.follow(url: link) { follow(f) }
                        }
                        .containerRelativeFrame([.horizontal, .vertical])
                        .id(Needs.id)
                        ForEach(tabs, id: \.id) { t in
                            page(t.id)
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
        .overlay {
            if switcher {
                TabSwitcher(client: client, selected: selected, shown: $switcher,
                            page: tabs.first { $0.id == selected }?.section.nilIfEmpty
                                ?? client.sections().first(where: \.chosen)?.id,
                            pick: pick)
                    .transition(.opacity)
            }
        }
        .sensoryFeedback(.impact, trigger: switcher) { _, now in now }
        .environment(\.nav, Nav(step: step, bell: bell))
        .onAppear {
            model.follow = follow
            settle(tabs)
        }
        .onChange(of: tabs.map(\.id)) { _, _ in settle(client.tabs()) }
        .onChange(of: selected) { _, now in
            client.openChannel(channel: now == Needs.id ? nil : now)
            if fallback {
                fallback = false
            } else {
                wanted = now
                UserDefaults.standard.set(now, forKey: "active")
            }
        }
        .sheet(isPresented: $showAgents) {
            let here = selected == Needs.id ? nil : selected
            AgentsView(client: client, channel: here, root: here.flatMap { threads[$0]?.root })
                .presentationDetents([.medium, .large])
                .presentationDragIndicator(.visible)
                .modifier(Themed())
        }
        .sheet(isPresented: $showChecklist) {
            if let s = selected {
                ChecklistView(client: client, channel: s)
                    .presentationDetents([.medium, .large])
                    .presentationDragIndicator(.visible)
                    .modifier(Themed())
            }
        }
        .sheet(isPresented: Binding(get: { quick != nil }, set: { if !$0 { quick = nil } })) {
            if let b = quick {
                let here = selected == Needs.id ? nil : selected
                QuickSheet(client: client, box: b, here: here, thread: here.flatMap { threads[$0]?.root }) { ch, id in
                    // Into the new thread, where the answer comes.
                    if let f = client.follow(url: "buzz://message?channel=\(ch)&id=\(id)&thread=\(id)") {
                        follow(f)
                    }
                }
                .presentationDetents([.medium, .large])
                .presentationDragIndicator(.visible)
                .modifier(Themed())
            }
        }
    }

    /// The Q button: the sheet, or why there's no #quick to ask in.
    private func openQuick() {
        do {
            quick = try client.openQuick(here: selected == Needs.id ? nil : selected)
        } catch let BuzzError.Failed(message) {
            model.say(message)
        } catch {
            model.say(error.localizedDescription)
        }
    }

    /// The tab `by` places along, wrapping (tab 0 included), on its open
    /// thread if it has one. The keyboard stays up on a channel tab.
    private func step(_ by: Int) {
        // Tab 0 (Needs you) sits before the first channel in the ring.
        let ring = [Needs.id] + client.tabs().map(\.id)
        let i = ring.firstIndex { $0 == selected } ?? 0
        let id = ring[((i + by) % ring.count + ring.count) % ring.count]
        if id != Needs.id {
            model.focusKey = threads[id].map { "\(id)/\($0.root)" } ?? id
        }
        go(id)
    }

    /// The bell key: the newest activity in this tab, read or not; a
    /// thread opens on its first new reply. The same place again until
    /// something newer arrives. Never leaves the tab.
    private func bell() {
        guard let s = selected, let j = client.bell(channel: s) else { return }
        land(s, j)
    }

    private func land(_ channel: String, _ j: Jump) {
        if let root = j.root {
            model.focusKey = "\(channel)/\(root)"
            show(channel, Route(channel: channel, root: root, focus: j.id))
        } else {
            model.focusKey = channel
            show(channel, nil)
            model.scrollTarget = (channel, j.id)
        }
    }

    /// The switcher comes down over everything, keyboard first.
    private func openSwitcher() {
        UIApplication.shared.sendAction(#selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil)
        withAnimation(.easeOut(duration: 0.15)) { switcher = true }
    }

    /// A tab picked in the switcher. One from a section the tab row isn't
    /// showing makes that section the tab row.
    private func pick(_ t: ChannelTab) {
        if !client.tabs().contains(where: { $0.section == t.section }) {
            model.setChosen([t.section])
        }
        go(t.id)
    }

    /// A tapped channel or message link: its tab (its section's tab row
    /// when it isn't showing), then its thread or the message.
    private func follow(_ f: Follow) {
        if let s = f.section {
            model.setChosen([s])
        } else if !client.tabs().contains(where: { $0.id == f.channel }) {
            model.say("that channel isn't in your sidebar")
            return
        }
        selected = f.channel
        if let root = f.root {
            show(f.channel, Route(channel: f.channel, root: root, focus: f.id))
        } else {
            show(f.channel, nil)
            if let id = f.id { model.scrollTarget = (f.channel, id) }
        }
    }

    /// Key-row moves only, so the keyboard is up: jump, no page slide.
    private func go(_ id: String) {
        selected = id
    }

    /// A tab's page: its open thread over its channel. The channel stays
    /// underneath, so going back finds it where it was.
    private func page(_ id: String) -> some View {
        ZStack {
            let thread = threads[id]
            ChannelView(client: client, channel: id) { root in
                show(id, Route(channel: id, root: root))
            }
            .opacity(thread == nil ? 1 : 0)
            .allowsHitTesting(thread == nil)
            if let r = thread {
                ThreadView(client: client, route: r, active: id == selected) { show(id, nil) }
                    .id(r)
                    .transition(.move(edge: .trailing))
            }
        }
        .clipped()
    }

    /// Open a thread in a tab, or close it (nil), and remember it.
    private func show(_ channel: String, _ r: Route?) {
        // The page underneath stays in the window: its box mustn't keep
        // the keyboard. A bell landing claims it back after.
        UIApplication.shared.sendAction(#selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil)
        withAnimation(.easeOut(duration: 0.2)) { threads[channel] = r }
        UserDefaults.standard.set(threads.mapValues(\.root), forKey: "threads")
    }

    private static func savedThreads() -> [String: Route] {
        let saved = UserDefaults.standard.dictionary(forKey: "threads") as? [String: String] ?? [:]
        return Dictionary(uniqueKeysWithValues: saved.map { ($0, Route(channel: $0, root: $1)) })
    }

    /// Keep the selection on a tab that exists, and tell Rust what's open.
    private func settle(_ tabs: [ChannelTab]) {
        let here = { (id: String) in id == Needs.id || tabs.contains(where: { $0.id == id }) }
        if let w = wanted, w != selected, here(w) {
            selected = w
        } else if let s = selected, here(s) {
            client.openChannel(channel: s == Needs.id ? nil : s)
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
    @Binding var showChecklist: Bool
    @Binding var switcher: Bool
    let openSwitcher: () -> Void
    let openQuick: () -> Void
    @Environment(Model.self) private var model
    @Environment(\.theme) private var theme
    /// Each tab's place in the row, to tell which one a hold landed on.
    @State private var chipFrames: [String: CGRect] = [:]

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 8) {
                status
                    .onTapGesture { showAgents = true }
                    .onLongPressGesture { openSwitcher() }
                Spacer()
                // Q: a quick question to taskbot in #quick.
                Button(action: openQuick) {
                    Text("Q")
                        .font(.chipOn)
                        .foregroundStyle(theme.muted)
                        .padding(.horizontal, 12)
                        .padding(.vertical, 4)
                        .background(theme.chipOff, in: RoundedRectangle(cornerRadius: 8))
                }
                .buttonStyle(.plain)
            }
            .padding(.horizontal, 22)
            // The operator loop's line for the open channel, when it has
            // one; ⚑ N (tab 0's count) in every channel's.
            let flags = client.needsCount()
            if let s = selected, s != Needs.id, let loop = client.loopStatus(channel: s) {
                LoopLine(status: loop, flags: flags)
                    .padding(.horizontal, 22)
                    .padding(.top, -8)
                    .contentShape(Rectangle())
                    .onTapGesture { showChecklist = true }
            } else if let s = selected, s != Needs.id, flags > 0 {
                Text("⚑ \(flags) need you")
                    .font(.small)
                    .foregroundStyle(theme.bright)
                    .padding(.horizontal, 22)
                    .padding(.top, -8)
                    .contentShape(Rectangle())
                    .onTapGesture { withAnimation(.easeOut(duration: 0.2)) { selected = Needs.id } }
            }
            ScrollViewReader { proxy in
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 8) {
                        needsChip(flags)
                            .id(Needs.id)
                            .onTapGesture { withAnimation(.easeOut(duration: 0.2)) { selected = Needs.id } }
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
        openSwitcher()
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
            Circle()
                .fill(selected == Needs.id ? theme.chipOn : theme.rule)
                .frame(width: 7, height: 7)
            ForEach(tabs, id: \.id) { t in
                Circle()
                    .fill(t.id == selected ? theme.chipOn : theme.rule)
                    .frame(width: 7, height: 7)
            }
        }
        .frame(maxWidth: .infinity)
    }

    /// Tab 0's chip, its badge the count of rows that need an answer.
    private func needsChip(_ n: UInt32) -> some View {
        let on = selected == Needs.id
        return HStack(spacing: 5) {
            Text("⚑ needs you")
                .font(on ? .chipOn : .chip)
                .lineLimit(1)
            if n > 0 {
                Text("\(n)")
                    .font(.tiny)
                    .foregroundStyle(theme.onAccent)
                    .padding(.horizontal, 5)
                    .frame(minWidth: 17, minHeight: 17)
                    .background(theme.badge, in: Capsule())
            }
        }
        .foregroundStyle(on ? theme.onAccent : theme.muted)
        .padding(.horizontal, 16)
        .padding(.vertical, 11)
        .background(on ? theme.chipOn : theme.chipOff, in: RoundedRectangle(cornerRadius: 10))
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

/// Every section's tabs in tall rows, a page per section. Swipe sideways
/// to look at another section; that alone changes nothing. Tap a tab to go
/// there (its section becomes the tab row), drag a handle to reorder, hold
/// one to archive it. On a section the tab you're on isn't in, the top row
/// moves it there. It flips down from the tab row and stays until a tab or
/// Close.
struct TabSwitcher: View {
    let client: Client
    let selected: String?
    @Binding var shown: Bool
    /// The section on screen: starts on the open tab's.
    @State var page: String?
    let pick: (ChannelTab) -> Void
    @Environment(Model.self) private var model
    @Environment(\.theme) private var theme
    @State private var open = false

    var body: some View {
        let _ = model.version
        let sections = client.sections()
        let name = sections.first { $0.id == page }?.name ?? ""
        VStack(spacing: 0) {
            HStack(spacing: 6) {
                Text(name).font(.label).foregroundStyle(theme.text).lineLimit(1)
                Spacer()
                ForEach(sections, id: \.id) { s in
                    Circle()
                        .fill(s.id == page ? theme.chipOn : theme.rule)
                        .frame(width: 7, height: 7)
                }
            }
            .padding(.horizontal, 22)
            .padding(.top, 14)
            .padding(.bottom, 6)
            if sections.isEmpty {
                Text(client.ready() ? "no sections" : "loading sections…")
                    .font(.small)
                    .foregroundStyle(theme.muted)
                    .frame(maxHeight: .infinity)
            }
            ScrollView(.horizontal) {
                LazyHStack(spacing: 0) {
                    ForEach(sections, id: \.id) { s in
                        SectionPage(client: client, section: s, selected: selected, pick: { t in
                            pick(t)
                            close()
                        })
                        .containerRelativeFrame([.horizontal, .vertical])
                        .id(s.id)
                    }
                }
                .scrollTargetLayout()
            }
            .scrollIndicators(.hidden)
            .scrollTargetBehavior(.paging)
            .scrollPosition(id: $page)
            HStack(spacing: 12) {
                Text("signed in as \(client.myName())")
                    .font(.small)
                    .foregroundStyle(theme.muted)
                    .lineLimit(1)
                Button("Log out") { model.logout() }
                    .font(.small)
                    .foregroundStyle(theme.badge)
                Spacer()
                Button("Close") { close() }
                    .font(.label)
                    .foregroundStyle(theme.accent)
            }
            .padding(.horizontal, 22)
            .padding(.vertical, 14)
        }
        .background(theme.canvas.ignoresSafeArea())
        // Flip down from the tab row.
        .rotation3DEffect(.degrees(open ? 0 : -70), axis: (1, 0, 0), anchor: .top, perspective: 0.6)
        .opacity(open ? 1 : 0)
        .onAppear { withAnimation(.spring(duration: 0.35)) { open = true } }
    }

    private func close() {
        withAnimation(.easeOut(duration: 0.2)) { shown = false }
    }
}

/// One section's tabs in the switcher.
private struct SectionPage: View {
    let client: Client
    let section: SectionInfo
    let selected: String?
    let pick: (ChannelTab) -> Void
    @Environment(Model.self) private var model
    @Environment(\.theme) private var theme
    /// The tab a hold asked to archive, until the confirm answers.
    @State private var archiving: ChannelTab?

    var body: some View {
        let _ = model.version
        let items = client.sectionTabs(section: section.id)
        let here = client.tabs().first { $0.id == selected }?.label ?? "#" + client.currentName()
        ScrollViewReader { proxy in
            List {
                if section.canMoveHere {
                    Button { model.moveHere(section.id) } label: {
                        Label("Move \(here) here", systemImage: "arrow.down.to.line")
                            .font(.label)
                            .foregroundStyle(theme.accent)
                            .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .modifier(SwitcherRow())
                }
                ForEach(items, id: \.id) { t in
                    Button { pick(t) } label: { row(t) }
                        .buttonStyle(.plain)
                        .id(t.id)
                        .modifier(SwitcherRow())
                        // Hold a row: archive it, after a confirm. (Rows
                        // are in drag mode and sideways swipes page the
                        // sections, so it isn't a swipe.)
                        .contextMenu {
                            Button(role: .destructive) { archiving = t } label: {
                                Label("Archive", systemImage: "archivebox")
                            }
                        }
                }
                .onMove { from, to in
                    var ids = items.map(\.id)
                    ids.move(fromOffsets: from, toOffset: to)
                    model.order(section.id, ids)
                }
                if items.isEmpty {
                    Text("no channels here")
                        .font(.small)
                        .foregroundStyle(theme.faint)
                        .modifier(SwitcherRow())
                }
            }
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
            // Edit mode for the drag handles; there's no delete.
            .environment(\.editMode, .constant(.active))
            .onAppear { proxy.scrollTo(selected, anchor: .center) }
            .confirmationDialog(
                "Archive \(archiving?.label ?? "") for everyone?",
                isPresented: Binding(get: { archiving != nil }, set: { if !$0 { archiving = nil } }),
                titleVisibility: .visible
            ) {
                Button("Archive", role: .destructive) {
                    if let t = archiving { model.archive(t.id) }
                    archiving = nil
                }
            }
        }
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

private struct SwitcherRow: ViewModifier {
    @Environment(\.theme) private var theme

    func body(content: Content) -> some View {
        content
            .listRowBackground(theme.canvas)
            .listRowSeparator(.hidden)
            .listRowInsets(EdgeInsets(top: 4, leading: 22, bottom: 4, trailing: 12))
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

private extension String {
    var nilIfEmpty: String? { isEmpty ? nil : self }
}
