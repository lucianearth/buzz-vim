import Foundation
import Security

/// The Keychain, this device only, in the access group the app shares with
/// its notification extension (`KeychainGroup` in both Info.plists, the
/// same group in both entitlements): the nsec ("nsec"), the relay ("relay")
/// and the push state ("push").
enum Keychain {
    private static let group = Bundle.main.object(forInfoDictionaryKey: "KeychainGroup") as? String

    private static func query(_ account: String, shared: Bool = true) -> [String: Any] {
        var q: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: "buzz-vim",
            kSecAttrAccount as String: account,
        ]
        if shared, let group { q[kSecAttrAccessGroup as String] = group }
        return q
    }

    static func save(_ value: String, account: String = "nsec") {
        delete(account: account)
        var q = query(account)
        q[kSecValueData as String] = Data(value.utf8)
        q[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        if SecItemAdd(q as CFDictionary, nil) == errSecMissingEntitlement {
            // Unsigned simulator builds have no access groups.
            q.removeValue(forKey: kSecAttrAccessGroup as String)
            SecItemAdd(q as CFDictionary, nil)
        }
    }

    static func read(account: String = "nsec") -> String? {
        if let v = copy(query(account)) { return v }
        // Builds before push kept the nsec outside the shared group: move it in.
        guard let v = copy(query(account, shared: false)) else { return nil }
        save(v, account: account)
        return v
    }

    /// Every copy, shared or not.
    static func delete(account: String = "nsec") {
        SecItemDelete(query(account, shared: false) as CFDictionary)
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
