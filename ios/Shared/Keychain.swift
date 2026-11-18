import Foundation
import Security

/// The Keychain, this device only. Two access groups:
/// - `.shared`, the group the app shares with its notification extension
///   (`KeychainGroup` in both Info.plists, the same group in both
///   entitlements): the nsec ("nsec"), the relay ("relay") and the push
///   state ("push").
/// - `.app`, the app's own application-identifier group, so only the app
///   reads it: pq keys ("pq:<id>", Pq.swift).
enum Keychain {
    enum Group { case shared, app }

    private static let shared = Bundle.main.object(forInfoDictionaryKey: "KeychainGroup") as? String
    /// `<team prefix>.<bundle id>`, the prefix taken from the shared group.
    private static let app: String? = {
        guard let shared, let dot = shared.firstIndex(of: "."),
              let bundle = Bundle.main.bundleIdentifier else { return nil }
        return String(shared[..<dot]) + "." + bundle
    }()

    /// In `group`, or (nil) in any group this app reads.
    private static func query(_ account: String, group: Group? = .shared) -> [String: Any] {
        var q: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: "buzz-vim",
            kSecAttrAccount as String: account,
        ]
        let name = switch group {
        case .shared: shared
        case .app: app
        case nil: String?.none
        }
        if let name { q[kSecAttrAccessGroup as String] = name }
        return q
    }

    /// Replace `account` with `value`; the Keychain's status. Readable
    /// after first unlock by default, so the extension can read the nsec
    /// while the phone is locked.
    @discardableResult
    static func save(_ value: String, account: String = "nsec", group: Group = .shared,
                     accessible: CFString = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly) -> OSStatus {
        delete(account: account)
        var q = query(account, group: group)
        q[kSecValueData as String] = Data(value.utf8)
        q[kSecAttrAccessible as String] = accessible
        var status = SecItemAdd(q as CFDictionary, nil)
        #if targetEnvironment(simulator)
        if status != errSecSuccess {
            // Unsigned simulator builds have no access groups (-34018 or
            // -25243). Never on a phone: there an item without a group
            // lands in the shared one.
            q.removeValue(forKey: kSecAttrAccessGroup as String)
            status = SecItemAdd(q as CFDictionary, nil)
        }
        #endif
        return status
    }

    static func read(account: String = "nsec", group: Group = .shared) -> String? {
        if let v = copy(query(account, group: group)) { return v }
        // Outside its group: a simulator fallback, or (shared) a build
        // before push kept the nsec outside the shared group: move it in.
        guard let v = copy(query(account, group: nil)) else { return nil }
        if group == .shared { save(v, account: account) }
        return v
    }

    /// `account` is here, in any group (its value is not read).
    static func has(account: String) -> Bool {
        var q = query(account, group: nil)
        q[kSecReturnAttributes as String] = true
        return SecItemCopyMatching(q as CFDictionary, nil) == errSecSuccess
    }

    /// Every copy, in any group. False when one was there and couldn't
    /// be deleted.
    @discardableResult
    static func delete(account: String = "nsec") -> Bool {
        let s = SecItemDelete(query(account, group: nil) as CFDictionary)
        return s == errSecSuccess || s == errSecItemNotFound
    }

    private static func copy(_ query: [String: Any]) -> String? {
        var q = query
        q[kSecReturnData as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var out: AnyObject?
        guard SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess,
              let data = out as? Data else { return nil }
        return String(data: data, encoding: .utf8)
    }
}

/// Where push stands for this phone, kept in the Keychain ("push") so the
/// notification extension can read what is leased.
struct PushState: Codable {
    /// The App Attest key and the gateway installation it enrolled, with
    /// the APNs token (hex) that installation delivers to.
    var keyId: String?
    var installation: String?
    var installationExpires: Int64 = 0
    var endpoint: String?
    /// The last delegation's generation, the relay key it went to, and its grant.
    var generation: Int64 = 0
    var relayPubkey: String?
    var grant: String?
    var grantExpires: Int64 = 0
    /// The lease's `d`: 16 random bytes, hex, for as long as the app is installed.
    var lease: String = (0..<16).map { _ in String(format: "%02x", UInt8.random(in: 0...255)) }.joined()
    var leaseGeneration: UInt64 = 0
    /// The subscriptions (JSON) the relay accepted, until when; nil when
    /// no lease is live.
    var leased: String?
    var leaseExpires: Int64 = 0
    /// A logout's stop the gateway hasn't confirmed yet: retried at launch
    /// and foreground. Optional, so state saved before it still decodes.
    var stopPending: Bool?

    static func load() -> PushState {
        Keychain.read(account: "push")
            .flatMap { try? JSONDecoder().decode(PushState.self, from: Data($0.utf8)) } ?? PushState()
    }

    func save() {
        if let data = try? JSONEncoder().encode(self) {
            Keychain.save(String(decoding: data, as: UTF8.self), account: "push")
        }
    }
}
