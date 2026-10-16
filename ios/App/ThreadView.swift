import OSLog
import SwiftUI

/// A thread, flat: the root, then the replies oldest first.
struct ThreadView: View {
    let client: Client
    let route: Route
    @Environment(Model.self) private var model
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    @State private var landed = false
    /// Where the bell key's reply is put: ScrollViewReader's scrollTo
    /// left the thread on its root.
    @State private var position = ScrollPosition(idType: String.self)

    var body: some View {
        let _ = model.version
        let tl = client.thread(channel: route.channel, root: route.root)
        let label = client.tabs().first { $0.id == route.channel }?.label ?? "thread"
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                Button { dismiss() } label: {
                    HStack(spacing: 10) {
                        Image(systemName: "arrow.left").font(.system(size: 16, weight: .medium))
                        Text(label).font(.header)
                    }
                    .foregroundStyle(theme.muted)
                }
                Spacer()
            }
            .padding(.horizontal, 22)
            .padding(.top, 10)
            .padding(.bottom, 14)
            GeometryReader { geo in
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 22) {
                        ForEach(tl.messages, id: \.id) { m in
                            MessageRow(client: client, message: m, showReplies: false,
                                       latest: m.id == tl.latest)
                                .contentShape(Rectangle())
                                .onTapGesture(count: 2) { client.thumbsUp(target: m.id) }
                                .id(m.id)
                        }
                        if !tl.working.isEmpty {
                            Working(names: tl.working)
                        }
                        // Room below, so a reply near the end can still sit at the top.
                        Color.clear.frame(height: route.focus == nil ? 1 : geo.size.height).id("end")
                    }
                    .scrollTargetLayout()
                    .padding(.horizontal, 22)
                    .padding(.top, 4)
                    // A short thread starts under the header, not at the bottom.
                    .frame(minHeight: geo.size.height, alignment: .top)
                }
                // Opened on a reply by the bell key: hold the top, so the
                // keyboard coming up doesn't push the reply off screen.
                .defaultScrollAnchor(route.focus == nil ? .bottom : .top)
                .scrollPosition($position, anchor: .top)
                .scrollDismissesKeyboard(.interactively)
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
        .toolbar(.hidden, for: .navigationBar)
        .safeAreaInset(edge: .bottom) {
            ComposeBar(client: client, channel: route.channel, root: route.root, hint: "reply…")
        }
        .onAppear { client.openThread(channel: route.channel, root: route.root) }
        .onDisappear {
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
