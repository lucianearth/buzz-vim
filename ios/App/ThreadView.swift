import SwiftUI

/// A thread, flat: the root, a rule, then the replies oldest first.
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
                    HStack(spacing: 8) {
                        Image(systemName: "chevron.left").font(.body.weight(.semibold))
                        Text(label).font(.body.weight(.semibold))
                    }
                    .foregroundStyle(theme.muted)
                }
                Spacer()
            }
            .padding(.horizontal, 18)
            .padding(.vertical, 10)
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 18) {
                        ForEach(Array(tl.messages.enumerated()), id: \.element.id) { i, m in
                            MessageRow(client: client, message: m, showReplies: false)
                                .contentShape(Rectangle())
                                .onTapGesture(count: 2) { client.thumbsUp(target: m.id) }
                            if i == 0 && tl.messages.count > 1 {
                                Rectangle().fill(theme.rule).frame(height: 1)
                            }
                        }
                        if !tl.working.isEmpty {
                            Working(names: tl.working)
                        }
                        Color.clear.frame(height: 1).id("end")
                    }
                    .padding(.horizontal, 18)
                    .padding(.top, 4)
                }
                .defaultScrollAnchor(.bottom)
                .scrollDismissesKeyboard(.interactively)
                .onChange(of: tl.messages.last?.id) { _, _ in
                    withAnimation(.easeOut(duration: 0.2)) { proxy.scrollTo("end", anchor: .bottom) }
                }
            }
        }
        .background(theme.canvas)
        .toolbar(.hidden, for: .navigationBar)
        .safeAreaInset(edge: .bottom) {
            ComposeBar(client: client, channel: route.channel, root: route.root, hint: "reply…")
        }
        .onAppear { client.openThread(channel: route.channel, root: route.root) }
    }
}
