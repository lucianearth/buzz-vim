import SwiftUI

/// A thread, flat: the root, then the replies oldest first.
struct ThreadView: View {
    let client: Client
    let route: Route
    @Environment(Model.self) private var model
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss

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
                        Color.clear.frame(height: 1).id("end")
                    }
                    .padding(.horizontal, 22)
                    .padding(.top, 4)
                    // A short thread starts under the header, not at the bottom.
                    .frame(minHeight: geo.size.height, alignment: .top)
                }
                .defaultScrollAnchor(.bottom)
                .scrollDismissesKeyboard(.interactively)
                .onChange(of: tl.messages.last?.id) { _, _ in
                    // Opened on a reply by the bell key: stay on it.
                    guard route.focus == nil else { return }
                    withAnimation(.easeOut(duration: 0.2)) { proxy.scrollTo("end", anchor: .bottom) }
                }
                .onAppear {
                    // The bell key: the first new reply at the top.
                    guard let f = route.focus else { return }
                    DispatchQueue.main.asyncAfter(deadline: .now() + 0.35) {
                        proxy.scrollTo(f, anchor: .top)
                    }
                }
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
}
