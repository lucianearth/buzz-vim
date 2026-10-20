import PhotosUI
import SwiftUI
import UIKit
import UniformTypeIdentifiers

/// One thin bar at the bottom. It grows as you type; the paperclip and the
/// send arrow show once you are writing. A photo uploads only after you say
/// yes on the sheet. Agents you @mention stay on as chips and every send
/// here goes to them until you tap one off. While typing, a row of keys
/// sits on the keyboard: @, #, code fence, tab moves, hide. Pasting an
/// image (a screenshot, a copied photo) opens the same upload sheet.
struct ComposeBar: View {
    let client: Client
    let channel: String
    let root: String?
    let hint: String
    @Environment(Model.self) private var model
    @Environment(\.theme) private var theme
    @Environment(\.nav) private var nav
    @State private var focused = false
    @State private var attachments: [Attachment] = []
    @State private var picked: PhotosPickerItem?
    @State private var pending: PendingUpload?
    @State private var uploading = false

    private var key: String { root.map { "\(channel)/\($0)" } ?? channel }

    private var text: Binding<String> {
        Binding(get: { model.drafts[key] ?? "" },
                set: { model.drafts[key] = $0.isEmpty ? nil : $0 })
    }

    private var canSend: Bool {
        !text.wrappedValue.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || !attachments.isEmpty
    }

    /// The sticky agents here. A thread nobody has changed yet starts with
    /// the agents its root addressed, as Desktop does.
    private var audience: [String] {
        model.sticky[key] ?? root.map { client.rootAgents(channel: channel, root: $0) } ?? []
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            suggestions
            if !audience.isEmpty {
                chips
            }
            if !attachments.isEmpty || uploading {
                HStack(spacing: 6) {
                    ForEach(attachments, id: \.url) { a in
                        HStack(spacing: 4) {
                            Image(systemName: "photo").font(.small)
                            Text("image").font(.small)
                            Image(systemName: "xmark").font(.tiny)
                        }
                        .foregroundStyle(theme.text)
                        .padding(.horizontal, 9).padding(.vertical, 5)
                        .background(theme.idle, in: Capsule())
                        .onTapGesture { attachments.removeAll { $0.url == a.url } }
                    }
                    if uploading {
                        ProgressView().controlSize(.small)
                        Text("uploading…").font(.small).foregroundStyle(theme.muted)
                    }
                }
                .padding(.horizontal, 6)
            }
            HStack(alignment: .bottom, spacing: 10) {
                if focused || canSend {
                    PhotosPicker(selection: $picked, matching: .images) {
                        Image(systemName: "paperclip")
                            .font(.text)
                            .foregroundStyle(theme.muted)
                            .padding(.bottom, 3)
                    }
                }
                ComposeField(text: text, focused: $focused, lines: 6) { data, type in
                    Task { prepare(await PendingUpload.make(data, type)) }
                }
                .overlay(alignment: .topLeading) {
                    if text.wrappedValue.isEmpty {
                        Text(hint)
                            .font(.text)
                            .foregroundStyle(theme.faint)
                            .allowsHitTesting(false)
                    }
                }
                if canSend {
                    Button(action: send) {
                        Image(systemName: "arrow.up.circle.fill")
                            .font(.title2)
                            .foregroundStyle(theme.accent)
                    }
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 16)
            .background(theme.card, in: RoundedRectangle(cornerRadius: 14))
            .overlay(RoundedRectangle(cornerRadius: 14).stroke(theme.rule, lineWidth: 1))
            if focused {
                keys
            }
        }
        .padding(.horizontal, 16)
        .padding(.top, 4)
        .padding(.bottom, 6)
        .background(theme.canvas)
        .animation(.easeOut(duration: 0.15), value: focused)
        // Tab moves from the key row land here with the keyboard still up.
        .onAppear { claimFocus() }
        .onChange(of: model.focusKey) { _, _ in claimFocus() }
        .onChange(of: picked) { _, item in
            guard let item else { return }
            picked = nil
            Task { prepare(await PendingUpload.load(item)) }
        }
        .sheet(item: $pending) { p in
            UploadSheet(upload: p, host: model.relayHost) { yes in
                pending = nil
                if yes { upload(p) }
            }
            .presentationDetents([.height(360)])
            .modifier(Themed())
        }
    }

    private func claimFocus() {
        guard model.focusKey == key else { return }
        model.focusKey = nil
        model.focusClaim = key
        // After a thread push settles, so the field is on screen to take it.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) {
            if model.focusClaim == key { focused = true }
        }
    }

    // ---------- sticky agents ----------

    private var chips: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                Text("to").font(.small).foregroundStyle(theme.muted)
                ForEach(audience, id: \.self) { pk in
                    let name = client.person(pubkey: pk).name
                    HStack(spacing: 4) {
                        Text("@" + name).font(.small).foregroundStyle(theme.ink(name))
                        Image(systemName: "xmark").font(.tiny).foregroundStyle(theme.muted)
                    }
                    .padding(.horizontal, 9).padding(.vertical, 5)
                    .background(theme.idle, in: Capsule())
                    .onTapGesture { model.sticky[key] = audience.filter { $0 != pk } }
                }
            }
            .padding(.horizontal, 6)
        }
    }

    // ---------- the keys on the keyboard ----------

    private var keys: some View {
        HStack(spacing: 0) {
            keyButton("at") { insert("@") }
            keyButton("number") { insert("#") }
            keyButton("chevron.left.forwardslash.chevron.right") { fence() }
            Spacer(minLength: 4)
            keyButton("chevron.left") { nav.step(-1) }
            keyButton("chevron.right") { nav.step(1) }
            keyButton("keyboard.chevron.compact.down") { focused = false }
        }
        .padding(.horizontal, 4)
    }

    private func keyButton(_ symbol: String, _ action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: symbol)
                .font(.system(size: 17))
                .foregroundStyle(theme.muted)
                .frame(width: 42, height: 34)
                .contentShape(Rectangle())
        }
    }

    /// Add `s` at the end, after a space if it would join the last word.
    private func insert(_ s: String) {
        var t = text.wrappedValue
        if let last = t.last, !last.isWhitespace { t += " " }
        text.wrappedValue = t + s
    }

    /// A fence line: the first opens a code block, the next closes it.
    private func fence() {
        var t = text.wrappedValue
        if let last = t.last, last != "\n" { t += "\n" }
        text.wrappedValue = t + "```\n"
    }

    // ---------- @ and # completion ----------

    /// The word being typed: from the last space or newline to the end.
    private var word: (range: Range<String.Index>, text: String)? {
        let t = text.wrappedValue
        let start = t.lastIndex { $0 == " " || $0 == "\n" }.map { t.index(after: $0) } ?? t.startIndex
        guard start < t.endIndex else { return nil }
        return (start..<t.endIndex, String(t[start...]))
    }

    @ViewBuilder
    private var suggestions: some View {
        if focused, let w = word, let first = w.text.first, first == "@" || first == "#" {
            let q = String(w.text.dropFirst()).lowercased()
            // Already named before the word being typed: not offered again.
            let before = text.wrappedValue[..<w.range.lowerBound]
            var seen = Set<String>()
            let names: [String] = (first == "@"
                ? client.people(channel: channel, root: root).map(\.name)
                    .filter { $0.lowercased().hasPrefix(q) && !Self.named($0, in: before) }
                : client.tabs().filter { !$0.isDm }.map { String($0.label.dropFirst()) }
                    .filter { $0.lowercased().hasPrefix(q) })
                .filter { seen.insert($0).inserted }
            if !names.isEmpty {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(names.prefix(5), id: \.self) { name in
                        Text(String(first) + name)
                            .font(.text)
                            .foregroundStyle(first == "@" ? theme.ink(name) : theme.text)
                            .padding(.vertical, 8)
                            .padding(.horizontal, 14)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .contentShape(Rectangle())
                            .onTapGesture { complete(String(first) + name) }
                    }
                }
                .background(theme.card, in: RoundedRectangle(cornerRadius: 14))
            }
        }
    }

    /// `@name` appears in `s` as a whole mention, not the start of a
    /// longer name (@Al inside @Alice).
    private static func named(_ name: String, in s: Substring) -> Bool {
        var rest = s[...]
        while let r = rest.range(of: "@" + name) {
            if r.upperBound == rest.endIndex || !rest[r.upperBound].isLetter && !rest[r.upperBound].isNumber {
                return true
            }
            rest = rest[r.upperBound...]
        }
        return false
    }

    private func complete(_ token: String) {
        guard let w = word else { return }
        var t = text.wrappedValue
        t.replaceSubrange(w.range, with: token + " ")
        text.wrappedValue = t
    }

    // ---------- send ----------

    private func send() {
        let body = text.wrappedValue
        let to = audience
        do {
            let id = try client.send(channel: channel, root: root, text: body, attachments: attachments, to: to)
            model.sent(id: id, key: key, text: body)
            let added = client.mentionedAgents(channel: channel, text: body).filter { !to.contains($0) }
            model.sticky[key] = to + added
            text.wrappedValue = ""
            attachments = []
        } catch let BuzzError.Failed(message) {
            model.say("not sent: \(message)")
        } catch {
            model.say("not sent: \(error.localizedDescription)")
        }
    }

    /// Ask about a prepared image, or say why there's nothing to ask about.
    private func prepare(_ p: PendingUpload?) {
        if let p {
            pending = p
        } else {
            model.say("couldn't read that image")
        }
    }

    private func upload(_ p: PendingUpload) {
        uploading = true
        Task {
            do {
                let a = try await client.upload(bytes: p.data, mime: p.mime)
                attachments.append(a)
                focused = true
            } catch let BuzzError.Failed(message) {
                model.say("upload failed: \(message)")
            } catch {
                model.say("upload failed: \(error.localizedDescription)")
            }
            uploading = false
        }
    }
}

/// A picked photo, read and ready, waiting for a yes.
struct PendingUpload: Identifiable {
    let id = UUID()
    let data: Data
    let mime: String
    let name: String
    let preview: UIImage?

    static func load(_ item: PhotosPickerItem) async -> PendingUpload? {
        guard let raw = try? await item.loadTransferable(type: Data.self) else { return nil }
        return await make(raw, item.supportedContentTypes.first)
    }

    /// The relay takes no metadata at all, so every image is redrawn and
    /// re-encoded clean (see Sanitize): PNG stays PNG, the rest go as JPEG.
    /// Off the main thread: a 48 MP photo takes a moment to redraw.
    static func make(_ raw: Data, _ type: UTType?) async -> PendingUpload? {
        await Task.detached(priority: .userInitiated) { build(raw, type) }.value
    }

    private static func build(_ raw: Data, _ type: UTType?) -> PendingUpload? {
        guard let img = UIImage(data: raw) else { return nil }
        let png = type?.conforms(to: .png) ?? false
        guard let data = Sanitize.image(img, png: png) else { return nil }
        let (mime, ext) = png ? ("image/png", "png") : ("image/jpeg", "jpg")
        return PendingUpload(data: data, mime: mime, name: "photo.\(ext)", preview: UIImage(data: data))
    }
}

struct UploadSheet: View {
    let upload: PendingUpload
    let host: String
    let done: (Bool) -> Void
    @Environment(\.theme) private var theme

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(spacing: 14) {
                if let img = upload.preview {
                    Image(uiImage: img)
                        .resizable()
                        .scaledToFill()
                        .frame(width: 76, height: 76)
                        .clipShape(RoundedRectangle(cornerRadius: 10))
                }
                VStack(alignment: .leading, spacing: 4) {
                    Text("\(upload.name) · \(ByteCountFormatter.string(fromByteCount: Int64(upload.data.count), countStyle: .file))")
                        .font(.button)
                        .foregroundStyle(theme.text)
                    Text("uploads to \(host.replacingOccurrences(of: "https://", with: ""))/upload")
                        .font(.small)
                        .foregroundStyle(theme.muted)
                }
            }
            Button { done(true) } label: {
                Text("Upload")
                    .font(.button)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 14)
                    .foregroundStyle(theme.onAccent)
                    .background(theme.accent, in: RoundedRectangle(cornerRadius: 14))
            }
            Button { done(false) } label: {
                Text("Cancel")
                    .font(.label)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 14)
                    .foregroundStyle(theme.text)
                    .background(theme.idle, in: RoundedRectangle(cornerRadius: 14))
            }
            Text("Nothing uploads until you say yes.")
                .font(.small)
                .foregroundStyle(theme.muted)
                .frame(maxWidth: .infinity)
        }
        .padding(22)
        .frame(maxHeight: .infinity, alignment: .top)
        .background(theme.sheet)
    }
}

/// The compose box: a UITextView, because SwiftUI's TextField drops a
/// pasted image without a word. Grows to `lines` lines, then scrolls.
/// Scrolling stays on the whole time and the height is measured on a
/// spare view: turning scrolling on and off as the text crossed the cap
/// threw the box between its first and last lines on every pass.
struct ComposeField: UIViewRepresentable {
    @Binding var text: String
    @Binding var focused: Bool
    let lines: Int
    let pasted: (Data, UTType) -> Void
    @Environment(\.theme) private var theme

    func makeUIView(context: Context) -> PasteTextView {
        let v = PasteTextView()
        v.delegate = context.coordinator
        v.font = UIFontMetrics(forTextStyle: .subheadline)
            .scaledFont(for: UIFont(name: "FiraMono-Regular", size: 15) ?? .monospacedSystemFont(ofSize: 15, weight: .regular))
        v.adjustsFontForContentSizeCategory = true
        v.backgroundColor = .clear
        v.textContainerInset = .zero
        v.textContainer.lineFragmentPadding = 0
        v.showsVerticalScrollIndicator = false
        v.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        return v
    }

    func updateUIView(_ v: PasteTextView, context: Context) {
        context.coordinator.parent = self
        v.pasted = pasted
        v.textColor = UIColor(theme.text)
        v.tintColor = UIColor(theme.accent)
        if v.text != text, v.markedTextRange == nil {
            v.text = text
            v.followCaret()
        }
        if focused, !v.isFirstResponder, v.window != nil {
            DispatchQueue.main.async { v.becomeFirstResponder() }
        } else if !focused, v.isFirstResponder {
            DispatchQueue.main.async { v.resignFirstResponder() }
        }
    }

    func sizeThatFits(_ proposal: ProposedViewSize, uiView v: PasteTextView, context: Context) -> CGSize? {
        // Never fall back to the view's own width: answering SwiftUI's
        // 0 and infinite probes with the current width made the box look
        // fixed, so the row stopped stretching it to fill the bar.
        let width = proposal.width.flatMap { $0.isFinite ? $0 : nil } ?? 240
        let line = v.font?.lineHeight ?? 18
        let most = line * CGFloat(lines)
        // SwiftUI's narrowest probe: laying the draft out a glyph per line
        // only to cap it at `lines` is wasted work on every keystroke.
        if width <= 1 { return CGSize(width: width, height: most) }
        let probe = Self.probe
        probe.font = v.font
        probe.attributedText = v.attributedText
        let fit = probe.sizeThatFits(CGSize(width: width, height: .greatestFiniteMagnitude))
        return CGSize(width: width, height: min(max(fit.height, line), most))
    }

    /// Measures the text, so sizing never touches the live view's scroll.
    private static let probe: UITextView = {
        let p = UITextView()
        p.isScrollEnabled = false
        p.textContainerInset = .zero
        p.textContainer.lineFragmentPadding = 0
        return p
    }()

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    final class Coordinator: NSObject, UITextViewDelegate {
        var parent: ComposeField
        init(_ parent: ComposeField) { self.parent = parent }

        func textViewDidChange(_ v: UITextView) {
            parent.text = v.text
        }
        func textViewDidBeginEditing(_ v: UITextView) { if !parent.focused { parent.focused = true } }
        func textViewDidEndEditing(_ v: UITextView) { if parent.focused { parent.focused = false } }
    }
}

/// Paste takes an image when the clipboard holds one; text pastes as usual.
final class PasteTextView: UITextView {
    var pasted: ((Data, UTType) -> Void)?

    /// After the text is set from outside (a completion, the @ key), show
    /// the caret. Typing and dictation keep it in view on their own.
    func followCaret() {
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.layoutIfNeeded()
            self.scrollRangeToVisible(self.selectedRange)
        }
    }

    /// While everything fits, sit at the top. Typing a new line scrolls
    /// it into view a moment before the box grows to hold it; without
    /// this the first line would stay hidden above.
    override func layoutSubviews() {
        super.layoutSubviews()
        if contentSize.height <= bounds.height + 1, contentOffset.y != 0 {
            contentOffset = .zero
        }
    }

    override func canPerformAction(_ action: Selector, withSender sender: Any?) -> Bool {
        if action == #selector(paste(_:)), UIPasteboard.general.hasImages { return true }
        return super.canPerformAction(action, withSender: sender)
    }

    override func paste(_ sender: Any?) {
        let pb = UIPasteboard.general
        guard pb.hasImages, let pasted else { return super.paste(sender) }
        // The bytes as copied when the type is one we know (a screenshot
        // is PNG), else the decoded image as PNG.
        for t in [UTType.png, .jpeg, .gif, .webP, .heic] {
            if let data = pb.data(forPasteboardType: t.identifier) {
                return pasted(data, t)
            }
        }
        if let data = pb.image?.pngData() {
            pasted(data, .png)
        }
    }
}
