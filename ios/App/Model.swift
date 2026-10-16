import Foundation
import Observation
import Security

/// The app's one piece of shared state: the Rust client and a version
/// counter. Views read snapshots from the client in `body` and touch
/// `version`, so every poke from Rust redraws whoever is on screen.
@Observable
final class Model {
    private(set) var client: Client?
    private(set) var version = 0
    var notice: String?
    var loginError: String?

    /// Compose boxes by conversation ("channel" or "channel/root"), kept
    /// across launches so nothing typed is lost.
    var drafts: [String: String] = UserDefaults.standard.dictionary(forKey: "drafts") as? [String: String] ?? [:] {
        didSet { UserDefaults.standard.set(drafts, forKey: "drafts") }
    }

    static let defaultRelay = "wss://buzz.gitcoin.co"

    var relayHost: String {
        let r = UserDefaults.standard.string(forKey: "relay") ?? Model.defaultRelay
        return r.replacingOccurrences(of: "wss://", with: "https://")
            .replacingOccurrences(of: "ws://", with: "http://")
    }

    init() {
        if let secret = Keychain.read() {
            let relay = UserDefaults.standard.string(forKey: "relay") ?? Model.defaultRelay
            try? start(relay: relay, secret: secret)
        } else if let secret = ProcessInfo.processInfo.environment["BUZZ_VIM_NSEC"], !secret.isEmpty {
            // Simulator runs log in from the environment (SIMCTL_CHILD_BUZZ_VIM_*);
            // an agent's key also needs its NIP-OA tag.
            let env = ProcessInfo.processInfo.environment
            try? start(relay: env["BUZZ_VIM_RELAY"] ?? Model.defaultRelay, secret: secret,
                       authTag: env["BUZZ_VIM_AUTH_TAG"])
        }
    }

    func login(relay: String, secret: String) {
        let relay = relay.trimmingCharacters(in: .whitespaces)
        do {
            _ = try pubkeyFor(secret: secret)
            try start(relay: relay.isEmpty ? Model.defaultRelay : relay, secret: secret)
            Keychain.save(secret.trimmingCharacters(in: .whitespacesAndNewlines))
            UserDefaults.standard.set(relay, forKey: "relay")
            loginError = nil
        } catch let BuzzError.Failed(message) {
            loginError = message
        } catch {
            loginError = error.localizedDescription
        }
    }

    func logout() {
        client?.flush()
        client = nil
        Keychain.delete()
        UserDefaults.standard.removeObject(forKey: "chosen")
        drafts = [:]
    }

    private func start(relay: String, secret: String, authTag: String? = nil) throws {
        let d = UserDefaults.standard
        let slot = d.string(forKey: "readSlot") ?? newReadId()
        let cid = d.string(forKey: "readClient") ?? newReadId()
        d.set(slot, forKey: "readSlot")
        d.set(cid, forKey: "readClient")
        let chosen = d.stringArray(forKey: "chosen") ?? []
        client = try Client(relayUrl: relay, secret: secret, chosen: chosen,
                            readSlot: slot, readClient: cid, authTag: authTag, listener: Poke(model: self))
    }

    func setChosen(_ ids: [String]) {
        UserDefaults.standard.set(ids, forKey: "chosen")
        client?.setChosen(ids: ids)
        bump()
    }

    private var pending = false

    /// From any thread: redraw soon, at most every 50 ms.
    func changed() {
        DispatchQueue.main.async {
            if self.pending { return }
            self.pending = true
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) {
                self.pending = false
                self.bump()
            }
        }
    }

    func bump() {
        version &+= 1
        if let n = client?.takeNotices(), let last = n.last {
            say(last)
        }
    }

    func say(_ s: String) {
        notice = s
        DispatchQueue.main.asyncAfter(deadline: .now() + 3.5) {
            if self.notice == s { self.notice = nil }
        }
    }
}

/// Rust calls this from its background loop.
final class Poke: Listener, @unchecked Sendable {
    weak var model: Model?
    init(model: Model) { self.model = model }
    func changed() { model?.changed() }
}

/// The nsec lives in the Keychain, this device only.
enum Keychain {
    private static let query: [String: Any] = [
        kSecClass as String: kSecClassGenericPassword,
        kSecAttrService as String: "buzz-vim",
        kSecAttrAccount as String: "nsec",
    ]

    static func save(_ secret: String) {
        delete()
        var q = query
        q[kSecValueData as String] = Data(secret.utf8)
        q[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        SecItemAdd(q as CFDictionary, nil)
    }

    static func read() -> String? {
        var q = query
        q[kSecReturnData as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var out: AnyObject?
        guard SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess,
              let data = out as? Data else { return nil }
        return String(data: data, encoding: .utf8)
    }

    static func delete() {
        SecItemDelete(query as CFDictionary)
    }
}
