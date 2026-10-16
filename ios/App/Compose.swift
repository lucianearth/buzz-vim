import ImageIO
import PhotosUI
import SwiftUI
import UIKit
import UniformTypeIdentifiers

/// One thin bar at the bottom. It grows as you type; the paperclip and the
/// send arrow show once you are writing. A photo uploads only after you say
/// yes on the sheet. Agents you @mention stay on as chips and every send
/// here goes to them until you tap one off. While typing, a row of keys
/// sits on the keyboard: @, #, code fence, tab moves, hide.
struct ComposeBar: View {
    let client: Client
    let channel: String
    let root: String?
    let hint: String
    @Environment(Model.self) private var model
    @Environment(\.theme) private var theme
    @Environment(\.nav) private var nav
    @FocusState private var focused: Bool
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
                TextField(hint, text: text, axis: .vertical)
                    .lineLimit(1...6)
                    .focused($focused)
                    .font(.text)
                    .foregroundStyle(theme.text)
                    .tint(theme.accent)
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
            Task { pending = await PendingUpload.load(item) }
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
            keyButton("bell.badge") { nav.nextUnread() }
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
        if focused, let w = word?.text, let first = w.first, first == "@" || first == "#" {
            let q = String(w.dropFirst()).lowercased()
            let names: [String] = first == "@"
                ? client.people(channel: channel).map(\.name).filter { $0.lowercased().hasPrefix(q) }
                : client.tabs().filter { !$0.isDm }.map { String($0.label.dropFirst()) }
                    .filter { $0.lowercased().hasPrefix(q) }
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

    /// The relay takes JPEG, PNG, GIF and WebP; anything else (HEIC) becomes JPEG.
    static func load(_ item: PhotosPickerItem) async -> PendingUpload? {
        guard let raw = try? await item.loadTransferable(type: Data.self) else { return nil }
        let type = item.supportedContentTypes.first
        let keep: [(UTType, String, String)] = [
            (.png, "image/png", "png"), (.gif, "image/gif", "gif"), (.webP, "image/webp", "webp"),
            (.jpeg, "image/jpeg", "jpg"),
        ]
        if let t = type, let k = keep.first(where: { t.conforms(to: $0.0) }),
           let data = withoutLocation(raw, k.0) {
            return PendingUpload(data: data, mime: k.1, name: "photo.\(k.2)", preview: UIImage(data: data))
        }
        guard let img = UIImage(data: raw), let jpg = img.jpegData(compressionQuality: 0.85) else { return nil }
        return PendingUpload(data: jpg, mime: "image/jpeg", name: "photo.jpg", preview: img)
    }

    /// The same image without GPS in its metadata (a lossless copy). JPEG
    /// and PNG only; GIF and WebP go as they are. Nil if ImageIO can't copy
    /// it, and then the caller re-encodes, which drops all metadata.
    private static func withoutLocation(_ data: Data, _ type: UTType) -> Data? {
        guard type.conforms(to: .jpeg) || type.conforms(to: .png) else { return data }
        guard let src = CGImageSourceCreateWithData(data as CFData, nil) else { return nil }
        let out = NSMutableData()
        guard let dst = CGImageDestinationCreateWithData(out, type.identifier as CFString, 1, nil) else { return nil }
        let opts: [CFString: Any] = [kCGImageMetadataShouldExcludeGPS: true]
        var err: Unmanaged<CFError>?
        guard CGImageDestinationCopyImageSource(dst, src, opts as CFDictionary, &err) else { return nil }
        return out as Data
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
