import SwiftUI
import UIKit

/// The operator loop's status line for a channel: mode, the watcher's
/// state, item counts, crashes today. Tap it for the checklist.
struct LoopLine: View {
    let status: LoopStatus
    @Environment(\.theme) private var theme

    var body: some View {
        HStack(spacing: 8) {
            if status.running {
                Text("RUNNING")
                    .font(.tiny)
                    .foregroundStyle(theme.onAccent)
                    .padding(.horizontal, 5)
                    .padding(.vertical, 1)
                    .background(theme.badge, in: RoundedRectangle(cornerRadius: 3))
            } else if let m = status.mode {
                Text(m).foregroundStyle(theme.muted)
            }
            if let w = status.watch {
                Text(watchText(w))
                    .foregroundStyle(status.needsYou ? theme.bright : theme.muted)
            }
            if status.open + status.done + status.parked > 0 {
                Text(counts).foregroundStyle(theme.muted)
            }
            if status.crashesToday > 0 {
                Text("✖ \(status.crashesToday) crash\(status.crashesToday == 1 ? "" : "es") today")
                    .foregroundStyle(theme.bright)
            }
        }
        .font(.small)
        .lineLimit(1)
        .truncationMode(.tail)
    }

    private var counts: String {
        var s = "\(status.open) open · \(status.done) done"
        if status.parked > 0 { s += " · \(status.parked) parked" }
        return s
    }

    private func watchText(_ w: String) -> String {
        guard let since = status.since else { return w }
        let now = UInt64(Date().timeIntervalSince1970)
        let secs = since > now ? 0 : now - since
        let ago = secs < 60 ? "\(secs)s" : secs < 3600 ? "\(secs / 60)m" : "\(secs / 3600)h"
        return "\(w) \(ago)"
    }
}

/// A channel's checklist, read-only: agents write it with `buzz checklist`.
/// Tap an item for its details, evidence and parked question.
struct ChecklistView: View {
    let client: Client
    let channel: String
    @Environment(Model.self) private var model
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    @State private var expanded: String?

    var body: some View {
        let _ = model.version
        let list = client.checklist(channel: channel)
        let status = client.loopStatus(channel: channel)
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 0) {
                if !list.title.isEmpty {
                    Text(list.title)
                        .font(.heading)
                        .foregroundStyle(theme.bright)
                }
                if !list.resume.isEmpty {
                    Text(list.resume)
                        .font(.small)
                        .foregroundStyle(theme.faint)
                        .padding(.top, 2)
                }
                if let s = status {
                    LoopLine(status: s).padding(.top, 8)
                    if let ping = s.pingLink {
                        Button("open the ping") { open(ping) }
                            .font(.small)
                            .foregroundStyle(theme.link)
                            .padding(.top, 4)
                    }
                }
                if list.items.isEmpty {
                    Text("no checklist in this channel")
                        .font(.text)
                        .foregroundStyle(theme.muted)
                        .padding(.top, 30)
                }
                ForEach(list.items, id: \.id) { it in
                    Rectangle().fill(theme.rule).frame(height: 1).padding(.vertical, 12)
                    row(it)
                        .contentShape(Rectangle())
                        .onTapGesture {
                            withAnimation(.easeOut(duration: 0.15)) {
                                expanded = expanded == it.id ? nil : it.id
                            }
                        }
                }
            }
            .padding(.horizontal, 22)
            .padding(.top, 26)
            .padding(.bottom, 20)
        }
        .background(theme.sheet)
    }

    private func row(_ it: ChecklistItem) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                Text(mark(it.status))
                    .foregroundStyle(it.status == .open ? theme.text : theme.muted)
                Text(it.text)
                    .foregroundStyle(it.status == .done ? theme.muted : theme.text)
                Spacer(minLength: 0)
            }
            .font(.text)
            let tags = flags(it)
            if !tags.isEmpty {
                Text(tags)
                    .font(.small)
                    .foregroundStyle(it.missingEvidence ? theme.bright : theme.faint)
                    .padding(.leading, 24)
            }
            if expanded == it.id {
                details(it).padding(.leading, 24).padding(.top, 6)
            }
        }
    }

    @ViewBuilder
    private func details(_ it: ChecklistItem) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("by \(it.creator)")
                .font(.small)
                .foregroundStyle(theme.muted)
            if let q = it.parkedQuestion {
                Text("? \(q)")
                    .font(.text)
                    .foregroundStyle(theme.bright)
            }
            if !it.evidence.isEmpty {
                Text("evidence").font(.small).foregroundStyle(theme.faint)
                ForEach(Array(it.evidence.enumerated()), id: \.offset) { _, e in
                    if let link = e.link {
                        Button(e.text) { open(link) }
                            .font(.mono)
                            .foregroundStyle(theme.link)
                            .lineLimit(1)
                    } else {
                        Text(e.text)
                            .font(.mono)
                            .foregroundStyle(theme.code)
                            .lineLimit(1)
                    }
                }
            }
            if !it.details.isEmpty {
                Text(markdown(it.details))
                    .font(.text)
                    .foregroundStyle(theme.text)
            }
        }
    }

    private func mark(_ s: ItemStatus) -> String {
        switch s {
        case .open: return "○"
        case .done: return "✓"
        case .parked: return "‖"
        }
    }

    private func flags(_ it: ChecklistItem) -> String {
        var f: [String] = []
        if it.release { f.append("release") }
        if it.status == .parked { f.append("parked") }
        if it.missingEvidence { f.append("⚠ no evidence") }
        return f.joined(separator: " · ")
    }

    private func markdown(_ s: String) -> AttributedString {
        let opts = AttributedString.MarkdownParsingOptions(interpretedSyntax: .inlineOnlyPreservingWhitespace)
        return (try? AttributedString(markdown: s, options: opts)) ?? AttributedString(s)
    }

    /// A message link opens here (closing the sheet); the rest as usual.
    private func open(_ link: String) {
        guard let url = URL(string: link) else { return }
        if client.follow(url: link) != nil {
            _ = model.open(url, client: client)
            dismiss()
        } else if client.isFile(url: link) {
            _ = model.open(url, client: client)
        } else {
            UIApplication.shared.open(url)
        }
    }
}
