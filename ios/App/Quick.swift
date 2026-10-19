import SwiftUI

/// The Q sheet, the terminal's Q box: a question for taskbot, posted as a
/// new thread in #quick (`[Q]` first, @taskbot). Tap Q again (the chip at
/// the top) to add the channel you were in and its latest messages; again
/// takes it off. The text stays if you close it. Sent, it lands you in the
/// new thread, where the answer comes.
struct QuickSheet: View {
    let client: Client
    let box: QuickBox
    /// The channel on screen and its open thread, for the second tap.
    let here: String?
    let thread: String?
    /// The new thread: (#quick id, message id).
    let sent: (String, String) -> Void
    @Environment(Model.self) private var model
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    @State private var include = false
    @FocusState private var focused: Bool

    private var text: Binding<String> {
        Binding(get: { model.drafts[QuickSheet.key] ?? "" },
                set: { model.drafts[QuickSheet.key] = $0.isEmpty ? nil : $0 })
    }

    /// Its compose box among the drafts; not a channel id.
    static let key = "quick"

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 8) {
                Button { if box.here != nil { include.toggle() } } label: {
                    Text(include ? "Q +" : "Q")
                        .font(.chipOn)
                        .foregroundStyle(theme.onAccent)
                        .padding(.horizontal, 14)
                        .padding(.vertical, 8)
                        .background(include ? theme.badge : theme.chipOn, in: RoundedRectangle(cornerRadius: 10))
                }
                .buttonStyle(.plain)
                Text("ask \(box.agent) in \(box.label)")
                    .font(.label)
                    .foregroundStyle(theme.text)
                    .lineLimit(1)
                Spacer()
            }
            Text(head)
                .font(.small)
                .foregroundStyle(include ? theme.bright : theme.faint)
            TextField("type your question", text: text, axis: .vertical)
                .font(.text)
                .lineLimit(3...10)
                .focused($focused)
                .padding(12)
                .background(theme.card, in: RoundedRectangle(cornerRadius: 14))
            HStack {
                Spacer()
                Button("Send") { send() }
                    .font(.button)
                    .foregroundStyle(theme.accent)
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 22)
        .padding(.top, 26)
        .background(theme.sheet)
        .onAppear { focused = true }
    }

    private var head: String {
        guard let name = box.here else { return "a new thread in \(box.label)" }
        return include
            ? "+ \(name): its id, link and latest 30 messages"
            : "a new thread in \(box.label). Tap Q again: + \(name)"
    }

    private func send() {
        do {
            let id = try client.askQuick(text: text.wrappedValue, here: include ? here : nil,
                                         thread: include ? thread : nil)
            model.sent(id: id, key: QuickSheet.key, text: text.wrappedValue)
            text.wrappedValue = ""
            dismiss()
            sent(box.channel, id)
        } catch let BuzzError.Failed(message) {
            model.say(message)
        } catch {
            model.say(error.localizedDescription)
        }
    }
}
