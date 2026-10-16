import PhotosUI
import SwiftUI
import UIKit
import UniformTypeIdentifiers

/// One thin bar at the bottom. It grows as you type; the paperclip and the
/// send arrow show once you are writing. A photo uploads only after you say
/// yes on the sheet.
struct ComposeBar: View {
    let client: Client
    let channel: String
    let root: String?
    let hint: String
    @Environment(Model.self) private var model
    @Environment(\.theme) private var theme
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

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            suggestions
            if !attachments.isEmpty || uploading {
                HStack(spacing: 6) {
                    ForEach(attachments, id: \.url) { a in
                        HStack(spacing: 4) {
                            Image(systemName: "photo").font(.caption)
                            Text("image").font(.caption)
                            Image(systemName: "xmark").font(.caption2)
                        }
                        .foregroundStyle(theme.text)
                        .padding(.horizontal, 9).padding(.vertical, 5)
                        .background(theme.idle, in: Capsule())
                        .onTapGesture { attachments.removeAll { $0.url == a.url } }
                    }
                    if uploading {
                        ProgressView().controlSize(.small)
                        Text("uploading…").font(.caption).foregroundStyle(theme.muted)
                    }
                }
                .padding(.horizontal, 6)
            }
            HStack(alignment: .bottom, spacing: 10) {
                if focused || canSend {
                    PhotosPicker(selection: $picked, matching: .images) {
                        Image(systemName: "paperclip")
                            .font(.body)
                            .foregroundStyle(theme.muted)
                            .padding(.bottom, 3)
                    }
                }
                TextField(hint, text: text, axis: .vertical)
                    .lineLimit(1...6)
                    .focused($focused)
                    .font(.body)
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
            .padding(.vertical, 17)
            .background(theme.card, in: RoundedRectangle(cornerRadius: 16))
        }
        .padding(.horizontal, 16)
        .padding(.top, 4)
        .padding(.bottom, 6)
        .background(theme.canvas)
        .animation(.easeOut(duration: 0.15), value: focused)
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

    // ---------- @ completion ----------

    private var query: String? {
        guard let last = text.wrappedValue.split(separator: " ", omittingEmptySubsequences: false).last,
              last.hasPrefix("@") else { return nil }
        return String(last.dropFirst()).lowercased()
    }

    @ViewBuilder
    private var suggestions: some View {
        if focused, let q = query {
            let people = client.people(channel: channel)
                .compactMap { line -> String? in
                    let name = line.split(separator: "\t").first.map(String.init) ?? ""
                    return name.lowercased().hasPrefix(q) ? name : nil
                }
                .prefix(4)
            if !people.isEmpty {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(Array(people), id: \.self) { name in
                        Text("@" + name)
                            .font(.subheadline)
                            .foregroundStyle(theme.ink(name))
                            .padding(.vertical, 8)
                            .padding(.horizontal, 14)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .contentShape(Rectangle())
                            .onTapGesture { complete(name) }
                    }
                }
                .background(theme.card, in: RoundedRectangle(cornerRadius: 14))
            }
        }
    }

    private func complete(_ name: String) {
        var words = text.wrappedValue.split(separator: " ", omittingEmptySubsequences: false).map(String.init)
        words.removeLast()
        words.append("@" + name + " ")
        text.wrappedValue = words.joined(separator: " ")
    }

    // ---------- send ----------

    private func send() {
        let body = text.wrappedValue
        do {
            let id = try client.send(channel: channel, root: root, text: body, attachments: attachments)
            model.sent(id: id, key: key, text: body)
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
        if let t = type, let k = keep.first(where: { t.conforms(to: $0.0) }) {
            return PendingUpload(data: raw, mime: k.1, name: "photo.\(k.2)", preview: UIImage(data: raw))
        }
        guard let img = UIImage(data: raw), let jpg = img.jpegData(compressionQuality: 0.85) else { return nil }
        return PendingUpload(data: jpg, mime: "image/jpeg", name: "photo.jpg", preview: img)
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
                        .font(.subheadline.weight(.semibold))
                        .foregroundStyle(theme.text)
                    Text("uploads to \(host.replacingOccurrences(of: "https://", with: ""))/upload")
                        .font(.footnote)
                        .foregroundStyle(theme.muted)
                }
            }
            Button { done(true) } label: {
                Text("Upload")
                    .font(.body.weight(.semibold))
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 14)
                    .foregroundStyle(theme.onAccent)
                    .background(theme.accent, in: RoundedRectangle(cornerRadius: 14))
            }
            Button { done(false) } label: {
                Text("Cancel")
                    .font(.body)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 14)
                    .foregroundStyle(theme.text)
                    .background(theme.idle, in: RoundedRectangle(cornerRadius: 14))
            }
            Text("Nothing uploads until you say yes.")
                .font(.footnote)
                .foregroundStyle(theme.muted)
                .frame(maxWidth: .infinity)
        }
        .padding(22)
        .frame(maxHeight: .infinity, alignment: .top)
        .background(theme.card)
    }
}
