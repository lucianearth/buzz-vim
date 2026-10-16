import SwiftUI

/// Who is working, where, and their last tool calls.
struct AgentsView: View {
    let client: Client
    @Environment(Model.self) private var model
    @Environment(\.theme) private var theme

    var body: some View {
        let _ = model.version
        let agents = client.agents()
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 0) {
                if agents.isEmpty {
                    Text("none of your agents are online")
                        .font(.footnote)
                        .foregroundStyle(theme.muted)
                        .padding(.top, 30)
                }
                ForEach(Array(agents.enumerated()), id: \.element.pubkey) { i, a in
                    if i > 0 {
                        Rectangle().fill(theme.rule).frame(height: 1).padding(.vertical, 14)
                    }
                    row(a)
                }
            }
            .padding(.horizontal, 22)
            .padding(.top, 26)
            .padding(.bottom, 20)
        }
        .background(theme.canvas)
    }

    private func row(_ a: Agent) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(a.name)
                .font(.title3.weight(.semibold))
                .foregroundStyle(theme.ink(a.pubkey))
            HStack(spacing: 6) {
                Pulse(color: a.busy ? theme.live : theme.faint, on: a.busy)
                Text(status(a))
                    .font(.footnote)
                    .foregroundStyle(theme.muted)
            }
            ForEach(Array(a.recent.suffix(3).enumerated()), id: \.offset) { _, step in
                Text(step)
                    .font(.mono)
                    .foregroundStyle(theme.faint)
                    .lineLimit(1)
                    .truncationMode(.tail)
            }
            if !a.thought.isEmpty {
                Text(a.thought)
                    .font(.footnote.italic())
                    .foregroundStyle(theme.muted)
                    .lineLimit(2)
            }
        }
    }

    private func status(_ a: Agent) -> String {
        guard a.busy else {
            return a.lastTurn.map { "idle · last turn \($0)" } ?? "idle"
        }
        var s = "working"
        if let p = a.place { s += " in \(p)" }
        if let secs = a.secs { s += secs < 60 ? " · \(secs)s" : " · \(secs / 60)m" }
        return s
    }
}
