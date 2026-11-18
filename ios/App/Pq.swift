import LocalAuthentication
import Security
import SwiftUI
import UIKit
import UniformTypeIdentifiers

/// pq keys: each request's record JSON (ffi `pq_request`) under account
/// `pq:<id>`, in the Keychain on this device only. Not in the group the
/// notification extension shares (Keychain.swift): the app's own
/// application-identifier group, so only the app reads them. Never synced.
enum PqKeys {
    private static let service = "buzz-vim-pq"

    /// `<team prefix>.<bundle id>`, the prefix taken from the shared group.
    private static let group: String? = {
        guard let shared = Bundle.main.object(forInfoDictionaryKey: "KeychainGroup") as? String,
              let dot = shared.firstIndex(of: "."),
              let bundle = Bundle.main.bundleIdentifier else { return nil }
        return String(shared[..<dot]) + "." + bundle
    }()

    private static func query(_ id: String) -> [String: Any] {
        [kSecClass as String: kSecClassGenericPassword,
         kSecAttrService as String: service,
         kSecAttrAccount as String: "pq:\(id)",
         kSecAttrSynchronizable as String: false]
    }

    /// Keep a new request's record; false when the Keychain said no.
    static func save(id: String, record: String) -> Bool {
        var q = query(id)
        if let group { q[kSecAttrAccessGroup as String] = group }
        q[kSecValueData as String] = Data(record.utf8)
        q[kSecAttrAccessible as String] = kSecAttrAccessibleWhenUnlockedThisDeviceOnly
        var status = SecItemAdd(q as CFDictionary, nil)
        #if targetEnvironment(simulator)
        if status == errSecMissingEntitlement {
            // Unsigned simulator builds have no access groups. Never on a
            // phone: without a group the item lands in the shared one.
            q.removeValue(forKey: kSecAttrAccessGroup as String)
            status = SecItemAdd(q as CFDictionary, nil)
        }
        #endif
        return status == errSecSuccess
    }

    /// This device holds the key for request `id` (no secret is read).
    static func has(_ id: String) -> Bool {
        var q = query(id)
        q[kSecReturnAttributes as String] = true
        return SecItemCopyMatching(q as CFDictionary, nil) == errSecSuccess
    }

    static func read(_ id: String) -> String? {
        var q = query(id)
        q[kSecReturnData as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var out: AnyObject?
        guard SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess,
              let data = out as? Data else { return nil }
        return String(data: data, encoding: .utf8)
    }

    /// False when a key was there and couldn't be deleted.
    @discardableResult
    static func burn(_ id: String) -> Bool {
        let s = SecItemDelete(query(id) as CFDictionary)
        return s == errSecSuccess || s == errSecItemNotFound
    }
}

/// Face ID (or the passcode) before a secret shows. A phone with neither
/// set can't open secrets.
@MainActor
func pqUnlock() async -> String? {
    let ctx = LAContext()
    var err: NSError?
    guard ctx.canEvaluatePolicy(.deviceOwnerAuthentication, error: &err) else {
        return "set a passcode or Face ID to open secrets"
    }
    do {
        return try await ctx.evaluatePolicy(.deviceOwnerAuthentication,
                                            localizedReason: "Open the sealed secret") ? nil : "not unlocked"
    } catch {
        return "not unlocked"
    }
}

/// A pq block in a message, drawn as a card like the terminal's: a REQUEST
/// with Send secret, a MESSAGE with Open (when this device holds its key).
struct PqCard: View {
    let client: Client
    let segment: Segment
    let message: Message
    /// Where a sealed reply goes: the channel, and the thread root.
    let channel: String
    let root: String
    @Environment(Model.self) private var model
    @Environment(\.theme) private var theme
    @State private var answering = false
    @State private var opened: PqOpened?

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            switch segment {
            case let .pqRequest(_, label, _):
                Text("🔒 \(message.authorName) asks for: \(label.isEmpty ? "a secret" : label)")
                    .font(.text)
                    .foregroundStyle(theme.bright)
                cardButton("Send secret", icon: "paperplane") { answering = true }
            case let .pqMessage(id, name, size, block, unasked):
                Text("🔐 sealed by \(message.authorName), \(name), "
                     + ByteCountFormatter.string(fromByteCount: Int64(size), countStyle: .file))
                    .font(.text)
                    .foregroundStyle(theme.bright)
                if !unasked.isEmpty {
                    Text("⚠ \(message.authorName) was not asked: the request named \(unasked)")
                        .font(.small)
                        .foregroundStyle(theme.badge)
                }
                if PqKeys.has(id) {
                    cardButton("Open", icon: "lock.open") { open(id: id, name: name, block: block, burn: true) }
                        .contextMenu {
                            Button("Open, keep the key") { open(id: id, name: name, block: block, burn: false) }
                        }
                } else {
                    Text("no key here (another device, or already opened)")
                        .font(.small)
                        .foregroundStyle(theme.faint)
                }
            default:
                EmptyView()
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(theme.card, in: RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).stroke(theme.rule, lineWidth: 1))
        .sheet(isPresented: $answering) {
            if case let .pqRequest(_, label, block) = segment {
                PqSendSheet(client: client, label: label, block: block, asker: message.author,
                            askerName: message.authorName, channel: channel, root: root)
                    .modifier(Themed())
            }
        }
        .fullScreenCover(item: $opened) { o in
            PqOpenedView(opened: o) { opened = nil }
                .modifier(Themed())
        }
    }

    private func cardButton(_ title: String, icon: String, _ action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Label(title, systemImage: icon)
                .font(.label)
                .padding(.horizontal, 14).padding(.vertical, 8)
                .foregroundStyle(theme.onAccent)
                .background(theme.accent, in: Capsule())
        }
        .buttonStyle(.plain)
    }

    private func open(id: String, name: String, block: String, burn: Bool) {
        Task { @MainActor in
            if let why = await pqUnlock() { return model.say(why) }
            guard let record = PqKeys.read(id) else {
                return model.say("no key here (another device, or already opened)")
            }
            do {
                let data = try pqOpen(messageBlock: block, record: record)
                // Burned as it opens, as the terminal and pq do: one look.
                if burn, !PqKeys.burn(id) { model.say("opened, but couldn't burn the key") }
                opened = PqOpened(name: name.isEmpty ? "message.out" : name, data: data)
            } catch let BuzzError.Failed(message) {
                model.say("couldn't open it: \(message)")
            } catch {
                model.say("couldn't open it: \(error.localizedDescription)")
            }
        }
    }
}

struct PqOpened: Identifiable {
    let id = UUID()
    let name: String
    let data: Data
}

/// What a sealed message held, over everything: copy (gone from the
/// pasteboard in a minute, never to other devices) or save (share sheet).
struct PqOpenedView: View {
    let opened: PqOpened
    let done: () -> Void
    @Environment(\.theme) private var theme
    @Environment(Model.self) private var model
    @Environment(\.scenePhase) private var phase

    /// Saved files wait here for the share sheet; emptied on each save and
    /// when the overlay closes, so a killed app leaves at most one behind.
    static let saves = FileManager.default.temporaryDirectory.appendingPathComponent("pq", isDirectory: true)

    var body: some View {
        let text = String(data: opened.data, encoding: .utf8)
        VStack(alignment: .leading, spacing: 16) {
            Text(opened.name).font(.button).foregroundStyle(theme.text)
            ScrollView {
                Text(text ?? "binary, \(opened.data.count) bytes: Save to keep it")
                    .font(.mono)
                    .foregroundStyle(text == nil ? theme.muted : theme.code)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .padding(12)
            .background(theme.codeBg, in: RoundedRectangle(cornerRadius: 10))
            HStack(spacing: 12) {
                if let text {
                    wide("Copy", filled: true) {
                        UIPasteboard.general.setItems(
                            [[UTType.utf8PlainText.identifier: text]],
                            options: [.expirationDate: Date().addingTimeInterval(60), .localOnly: true])
                        model.say("copied; the pasteboard forgets it in a minute")
                    }
                }
                wide("Save", filled: text == nil) { save() }
            }
            wide("Done", filled: false, action: close)
            Text("The key is gone from this phone unless you kept it.")
                .font(.small)
                .foregroundStyle(theme.muted)
                .frame(maxWidth: .infinity)
        }
        .padding(22)
        .background(theme.sheet)
        // Not into the app switcher's snapshot.
        .onChange(of: phase) { _, now in if now == .background { close() } }
    }

    private func close() {
        try? FileManager.default.removeItem(at: Self.saves)
        done()
    }

    private func wide(_ title: String, filled: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title)
                .font(filled ? .button : .label)
                .frame(maxWidth: .infinity)
                .padding(.vertical, 14)
                .foregroundStyle(filled ? theme.onAccent : theme.text)
                .background(filled ? theme.accent : theme.idle, in: RoundedRectangle(cornerRadius: 14))
        }
    }

    /// To a file only the share sheet sees, deleted when it closes. The
    /// sender chose the name: its last path part, no leading dots.
    private func save() {
        try? FileManager.default.removeItem(at: Self.saves)
        let dir = Self.saves.appendingPathComponent(UUID().uuidString, isDirectory: true)
        var name = String(opened.name.drop { $0 == "." })
        if name.isEmpty { name = "message.out" }
        let file = dir.appendingPathComponent(name)
        do {
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            try opened.data.write(to: file, options: [.withoutOverwriting, .completeFileProtection])
        } catch {
            return model.say("couldn't save: \(error.localizedDescription)")
        }
        model.share(file) { try? FileManager.default.removeItem(at: dir) }
    }
}

/// Send secret on a REQUEST: paste it (say from 1Password), type it, or
/// pick a file; then the same yes/no sheet uploads use; then the sealed
/// MESSAGE goes into the request's thread, tagging who asked.
struct PqSendSheet: View {
    let client: Client
    let label: String
    let block: String
    let asker: String
    let askerName: String
    let channel: String
    let root: String
    @Environment(Model.self) private var model
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    @State private var typed = ""
    @State private var picking = false
    @State private var pending: PendingUpload?
    /// Said yes on the confirm sheet: sealed and sent once it has gone.
    @State private var approved: PendingUpload?

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Send \(askerName) \(label.isEmpty ? "a secret" : label)")
                .font(.button)
                .foregroundStyle(theme.text)
            PasteButton(payloadType: String.self) { strings in
                guard let s = strings.first, !s.isEmpty else { return }
                Task { @MainActor in confirm(Data(s.utf8), name: "message.txt") }
            }
            .labelStyle(.titleAndIcon)
            SecureField("or type it", text: $typed)
                .font(.text)
                .padding(12)
                .background(theme.idle, in: RoundedRectangle(cornerRadius: 10))
                .onSubmit { if !typed.isEmpty { confirm(Data(typed.utf8), name: "message.txt") } }
            Button { picking = true } label: {
                Label("Choose a file", systemImage: "doc").font(.label)
            }
            Text("Sealed on this phone for the key that asked; nobody else, the relay included, can read it.")
                .font(.small)
                .foregroundStyle(theme.muted)
        }
        .padding(22)
        .frame(maxHeight: .infinity, alignment: .top)
        .background(theme.sheet)
        .presentationDetents([.medium])
        .fileImporter(isPresented: $picking, allowedContentTypes: [.item]) { result in
            switch result {
            case let .success(url):
                let scoped = url.startAccessingSecurityScopedResource()
                defer { if scoped { url.stopAccessingSecurityScopedResource() } }
                let size = (try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0
                // Inline in a message: keys, tokens, .env files; not media.
                if size > 1 << 20 { return model.say("too big to send in a message (over 1 MB)") }
                do {
                    confirm(try Data(contentsOf: url), name: url.lastPathComponent)
                } catch {
                    model.say("couldn't read it: \(error.localizedDescription)")
                }
            case let .failure(e):
                model.say("couldn't read it: \(e.localizedDescription)")
            }
        }
        .sheet(item: $pending, onDismiss: {
            if let p = approved {
                approved = nil
                send(p)
            }
        }) { p in
            UploadSheet(upload: p, host: model.relayHost,
                        action: "Seal and send",
                        detail: "sealed for \(askerName), as a reply in the thread",
                        footer: "Nothing is sent until you say yes.") { yes in
                if yes { approved = p }
                pending = nil
            }
            .presentationDetents([.height(320)])
            .modifier(Themed())
        }
    }

    private func confirm(_ data: Data, name: String) {
        if data.isEmpty { return model.say("nothing to send") }
        if data.count > 1 << 20 { return model.say("too big to send in a message (over 1 MB)") }
        pending = PendingUpload(data: data, mime: "", name: name, preview: nil)
    }

    private func send(_ p: PendingUpload) {
        do {
            let sealed = try pqSeal(requestBlock: block, data: p.data, name: p.name)
            _ = try client.send(channel: channel, root: root, text: sealed, attachments: [], to: [asker])
            typed = ""
            dismiss()
        } catch let BuzzError.Failed(message) {
            model.say("not sent: \(message)")
        } catch {
            model.say("not sent: \(error.localizedDescription)")
        }
    }
}

/// Request secret: a REQUEST block posted here, its key kept in PqKeys
/// first (no key, no post).
func pqAsk(client: Client, model: Model, channel: String, root: String?, label: String) {
    let new = pqRequest(label: label.trimmingCharacters(in: .whitespacesAndNewlines))
    guard PqKeys.save(id: new.id, record: new.record) else {
        return model.say("couldn't keep the key; nothing posted")
    }
    do {
        _ = try client.send(channel: channel, root: root, text: new.block, attachments: [], to: [])
    } catch let BuzzError.Failed(message) {
        PqKeys.burn(new.id)
        model.say("not sent: \(message)")
    } catch {
        PqKeys.burn(new.id)
        model.say("not sent: \(error.localizedDescription)")
    }
}
