import OSLog
import SwiftUI

/// A thread, flat: the root, then the replies oldest first. It sits in
/// its channel's tab, under the tab row; the floating arrow bottom left
/// goes back to the channel.
struct ThreadView: View {
    let client: Client
    let route: Route
    /// Its tab is the one on screen (a neighbour page may be built early).
    let active: Bool
    let back: () -> Void
    @Environment(Model.self) private var model
    @Environment(\.theme) private var theme
    @State private var landed = false
    /// Where the bell key's reply is put: ScrollViewReader's scrollTo
    /// left the thread on its root.
    @State private var position = ScrollPosition(idType: String.self)
    @State private var edges = Edges()
    @State private var fling = Fling()

    var body: some View {
        let _ = model.version
        let tl = client.thread(channel: route.channel, root: route.root)
        VStack(spacing: 0) {
            GeometryReader { geo in
            ScrollViewReader { proxy in
                ScrollView {
                    // Not lazy, for the same reason as the channel view.
                    VStack(alignment: .leading, spacing: 22) {
                        ForEach(tl.messages, id: \.id) { m in
                            MessageRow(client: client, message: m, showReplies: false,
                                       latest: m.id == tl.latest)
                                .equatable()
                                .contentShape(Rectangle())
                                .onTapGesture(count: 2) { client.thumbsUp(target: m.id) }
                                .id(m.id)
                        }
                        if !tl.working.isEmpty {
                            Working(names: tl.working)
                        }
                        Color.clear.frame(height: 1).id("end")
                    }
                    .scrollTargetLayout()
                    .padding(.horizontal, 22)
                    .padding(.top, 4)
                    // A short thread starts under the header, not at the bottom.
                    .frame(minHeight: geo.size.height, alignment: .top)
                    .background(FlingStop(fling: fling))
                }
                // Opened on a reply by the bell key: hold the top, so the
                // keyboard coming up doesn't push the reply off screen.
                .defaultScrollAnchor(route.focus == nil ? .bottom : .top)
                .defaultScrollAnchor(route.focus == nil ? .bottom : .top, for: .sizeChanges)
                .scrollPosition($position, anchor: .top)
                .scrollDismissesKeyboard(.interactively)
                .onScrollGeometryChange(for: Edges.self, of: Edges.init) { _, now in edges = now }
                .overlay(alignment: .bottomTrailing) {
                    EdgeButtons(edges: edges) { top in
                        fling.stop()
                        withAnimation(.easeOut(duration: 0.25)) { position.scrollTo(edge: top ? .top : .bottom) }
                    }
                }
                .overlay(alignment: .bottomLeading) {
                    Button(action: back) {
                        Image(systemName: "arrow.left")
                            .font(.system(size: 15, weight: .semibold))
                            .foregroundStyle(theme.text)
                            .frame(width: 38, height: 38)
                            .background(theme.idle, in: Circle())
                            .overlay(Circle().stroke(theme.rule, lineWidth: 1))
                    }
                    .padding(.leading, 14)
                    .padding(.bottom, 10)
                }
                .onChange(of: tl.messages.last?.id) { _, _ in
                    // Opened on a reply by the bell key: stay on it.
                    guard route.focus == nil else { return }
                    withAnimation(.easeOut(duration: 0.2)) { proxy.scrollTo("end", anchor: .bottom) }
                }
                // The bell key: the first new reply at the top, once it has
                // loaded, and again after the push and keyboard settle.
                .onAppear { land(tl) }
                .onChange(of: tl.messages.count) { _, _ in land(tl) }
            }
            }
        }
        .background(theme.canvas)
        .safeAreaInset(edge: .bottom) {
            ComposeBar(client: client, channel: route.channel, root: route.root, hint: "reply…")
        }
        // Open (load, mark read) only while its tab is on screen.
        .onChange(of: active, initial: true) { was, now in
            if now {
                client.openThread(channel: route.channel, root: route.root)
            } else if was {
                client.closeThread(root: route.root)
                model.bump()
            }
        }
        .onDisappear {
            guard active else { return }
            client.closeThread(root: route.root)
            model.bump()
        }
    }

    private func land(_ tl: Timeline) {
        guard !landed, let f = route.focus, tl.messages.contains(where: { $0.id == f }) else { return }
        landed = true
        let i = tl.messages.firstIndex { $0.id == f } ?? -1
        Logger(subsystem: "com.lucianhymer.buzzvim", category: "bell")
            .info("thread \(route.root, privacy: .public): reply \(f, privacy: .public) at \(i) of \(tl.messages.count)")
        for delay in [0.05, 0.35, 0.7] {
            DispatchQueue.main.asyncAfter(deadline: .now() + delay) { position.scrollTo(id: f, anchor: .top) }
        }
    }
}
