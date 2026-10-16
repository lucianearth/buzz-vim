import SwiftUI

struct Route: Hashable {
    let channel: String
    let root: String
}

/// Tabs across the top, one channel per page: swipe sideways between them,
/// tap a message for its thread, pull the header down for the agents.
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

    var body: some View {
        let _ = model.version
        let tabs = client.tabs()
        NavigationStack(path: $path) {
            VStack(spacing: 0) {
                Header(client: client, tabs: tabs, selected: $selected,
                       showAgents: $showAgents, showSections: $showSections)
                if tabs.isEmpty {
                    Spacer()
                    Text(client.ready() ? "no channels yet" : "loading channels…")
                        .font(.footnote)
                        .foregroundStyle(theme.muted)
                    Spacer()
                } else {
                    TabView(selection: $selected) {
                        ForEach(tabs, id: \.id) { t in
                            ChannelView(client: client, channel: t.id) { root in
                                path.append(Route(channel: t.id, root: root))
                            }
                            .tag(Optional(t.id))
                        }
                    }
                    .tabViewStyle(.page(indexDisplayMode: .never))
                    // A paged TabView whose pages change under a restored
                    // selection can show a blank page; rebuild it instead.
                    .id(tabs.map(\.id))
                }
            }
            .background(theme.canvas)
            .toolbar(.hidden, for: .navigationBar)
            .navigationDestination(for: Route.self) { r in
                ThreadView(client: client, route: r)
            }
        }
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
    @Environment(\.theme) private var theme

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            status
                .padding(.horizontal, 22)
                .onTapGesture { showAgents = true }
            ScrollViewReader { proxy in
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 8) {
                        ForEach(tabs, id: \.id) { t in
                            chip(t)
                                .id(t.id)
                                .onTapGesture { withAnimation(.easeOut(duration: 0.2)) { selected = t.id } }
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
            .simultaneousGesture(LongPressGesture(minimumDuration: 0.5).onEnded { _ in
                showSections = true
            })
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
                .font(.subheadline)
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
                .font(.subheadline.weight(on ? .semibold : .regular))
                .lineLimit(1)
            if t.unread > 0 && !on {
                Circle().fill(theme.badge).frame(width: 6, height: 6)
            }
        }
        .foregroundStyle(on ? theme.onAccent : theme.muted)
        .padding(.horizontal, 16)
        .padding(.vertical, 11)
        .background(on ? theme.chipOn : theme.chipOff, in: RoundedRectangle(cornerRadius: 10))
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
