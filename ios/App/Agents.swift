import SwiftUI

/// Who is working, where, and their last tool calls; context in the open
/// channel, and Reset to start an agent's session there afresh.
struct AgentsView: View {
    let client: Client
    /// The channel on screen, and its open thread.
    let channel: String?
    let root: String?
    @Environment(Model.self) private var model
    @Environment(\.theme) private var theme
    @State private var resetting: Agent?

    var body: some View {
        let _ = model.version
        let agents = client.agents()
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 0) {
                if agents.isEmpty {
                    Text("none of your agents are online")
                        .font(.text)
                        .foregroundStyle(theme.muted)
                        .padding(.top, 30)
                }
                ForEach(Array(agents.enumerated()), id: \.element.pubkey) { i, a in
                    if i > 0 {
                        Rectangle().fill(theme.rule).frame(height: 1).padding(.vertical, 20)
                    }
                    row(a)
                }
            }
            .padding(.horizontal, 22)
            .padding(.top, 26)
            .padding(.bottom, 20)
        }
        .background(theme.sheet)
        .confirmationDialog(
            "Reset \(resetting?.name ?? "")'s session here?",
            isPresented: Binding(get: { resetting != nil }, set: { if !$0 { resetting = nil } }),
            titleVisibility: .visible
        ) {
            Button("Reset", role: .destructive) {
                if let a = resetting, let ch = channel {
                    do {
                        try client.resetAgent(channel: ch, root: root, agent: a.pubkey)
                        model.say("reset sent to \(a.name)")
                    } catch {
                        model.say("reset not sent: \(error.localizedDescription)")
                    }
                }
                resetting = nil
            }
        }
    }

    private func row(_ a: Agent) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(alignment: .firstTextBaseline) {
                Text(a.name)
                    .font(.heading)
                    .foregroundStyle(theme.ink(a.pubkey))
                Spacer()
                if channel != nil {
                    Button("Reset") { resetting = a }
                        .font(.small)
                        .foregroundStyle(theme.muted)
                }
            }
            if let c = a.context {
                Text("context here \(c)")
                    .font(.small)
                    .foregroundStyle(theme.muted)
            }
            HStack(spacing: 7) {
                if a.busy { Pulse(color: theme.live, on: true) }
                Text(status(a))
                    .font(.text)
                    .foregroundStyle(theme.muted)
            }
            let steps = a.recent.suffix(3)
            if !steps.isEmpty || !a.thought.isEmpty {
                Rectangle().fill(theme.rule).frame(height: 1).padding(.vertical, 8)
            }
            // "bash  cargo test": the tool in muted, what it ran in code green.
            ForEach(Array(steps.enumerated()), id: \.offset) { _, step in
                let parts = step.split(separator: " ", maxSplits: 1).map(String.init)
                HStack(alignment: .firstTextBaseline, spacing: 12) {
                    Text(parts.first ?? "")
                        .foregroundStyle(theme.muted)
                        .frame(minWidth: 44, alignment: .leading)
                    if parts.count > 1 {
                        Text(parts[1]).foregroundStyle(theme.code)
                    }
                }
                .font(.mono)
                .lineLimit(1)
                .truncationMode(.tail)
                .padding(.leading, 14)
            }
            if !a.thought.isEmpty {
                Text(a.thought)
                    .font(.small)
                    .foregroundStyle(theme.muted)
                    .lineLimit(2)
                    .padding(.leading, 14)
                    .padding(.top, 2)
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
