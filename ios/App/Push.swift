import DeviceCheck
import Foundation
import UIKit

/// Push to the closed app, on upstream Buzz's native push (see
/// `buzz_vim_core::push`). The APNs token is enrolled with the push gateway
/// under an App Attest key, delegated to the relay's push key, and the
/// relay gets a lease saying which events wake the phone. APNs carries no
/// content: the notification extension fetches it.
///
/// Renewal: the grant lasts 29 days and is renewed when a week is left,
/// whenever the app runs. The lease follows the subscriptions (DMs, bots)
/// and is published again when they change.
final class Push {
    static let shared = Push()
    /// The push gateway this app enrolls with.
    static let gateway = "https://push.buzz.gitcoin.co"
    /// Inside the gateway's 30-day grant cap, with room for clock skew.
    private static let lifetime: Int64 = 29 * 86_400

    private var state = PushState.load()
    private var token: Data?
    private var busy = false
    private var checked = Date.distantPast

    /// A lease the relay took is in force: tab 0 posts no local
    /// notifications then, the push covers them.
    var live: Bool {
        state.leased != nil && state.leaseExpires > Self.now()
    }

    private static func now() -> Int64 { Int64(Date().timeIntervalSince1970) }

    /// From the app delegate (main thread).
    func registered(token: Data) {
        self.token = token
        checked = .distantPast
    }

    /// On sync pokes (main thread): lease, renew or re-lease when due. At
    /// most every 30 seconds, one attempt at a time.
    func refresh(_ client: Client, relay: String) {
        guard !busy, Date().timeIntervalSince(checked) > 30, let token,
              DCAppAttestService.shared.isSupported,
              let subs = try? client.pushSubscriptions() else { return }
        checked = Date()
        let now = Self.now()
        let endpoint = token.map { String(format: "%02x", $0) }.joined()
        let renew = state.grant == nil || state.endpoint != endpoint || state.grantExpires < now + 7 * 86_400
        guard renew || state.leased != subs || !live else { return }
        busy = true
        Task { @MainActor in
            defer { busy = false }
            do {
                let r = try await pushRelay(relayUrl: relay)
                if renew || state.relayPubkey != r.pubkey {
                    try await delegate(to: r, endpoint: endpoint)
                }
                guard let grant = state.grant else { return }
                let generation = max(state.leaseGeneration + 1, UInt64(Self.now()))
                try await client.pushLease(relay: r, installation: state.lease, generation: generation,
                                           grant: grant, grantExpires: UInt64(state.grantExpires),
                                           subscriptions: subs)
                state.leaseGeneration = generation
                state.leased = subs
                state.leaseExpires = min(state.grantExpires, Self.now() + Int64(r.maxTtl))
                state.save()
            } catch {
                // Not again for ten minutes (a relay without push, no network).
                checked = Date().addingTimeInterval(600)
                NSLog("push: \(error)")
            }
        }
    }

    /// Logout: an inactive lease, then the gateway forgets the relay key.
    /// The App Attest key and installation stay for the next login.
    func stop(_ client: Client, relay: String) {
        guard state.leased != nil || state.grant != nil else { return }
        let was = state
        state.leased = nil
        state.grant = nil
        state.save()
        Task { @MainActor in
            do {
                let r = try await pushRelay(relayUrl: relay)
                let generation = max(was.leaseGeneration + 1, UInt64(Self.now()))
                try await client.pushTombstone(relay: r, installation: was.lease, generation: generation)
                state.leaseGeneration = generation
                state.save()
                guard let keyId = was.keyId, let installation = was.installation,
                      let relayPubkey = was.relayPubkey else { return }
                let d = PushDelegation(installation: installation, generation: was.generation,
                                       relayPubkey: relayPubkey, notBefore: 0, expiresAt: 0)
                let c = try await pushChallenge(gateway: Self.gateway)
                let assertion = try await DCAppAttestService.shared.generateAssertion(
                    keyId, clientDataHash: pushRevokeHash(challenge: c, delegation: d))
                try await pushRevoke(gateway: Self.gateway, challenge: c, delegation: d, assertion: assertion)
            } catch {
                NSLog("push stop: \(error)")
            }
        }
    }

    /// A fresh grant for the relay key, enrolling first when there is no
    /// installation for this token (or it is about to lapse, or the gateway
    /// forgot it).
    @MainActor
    private func delegate(to r: PushRelay, endpoint: String, again: Bool = true) async throws {
        if state.installation == nil || state.endpoint != endpoint || state.installationExpires <= Self.now() + 300 {
            try await enroll(endpoint)
        }
        guard let keyId = state.keyId, let installation = state.installation else { return }
        // Kept before asking: a delegation whose answer got lost still
        // used up its generation.
        state.generation += 1
        state.save()
        let now = Self.now()
        let d = PushDelegation(installation: installation, generation: state.generation,
                               relayPubkey: r.pubkey, notBefore: now, expiresAt: now + Self.lifetime)
        let c = try await pushChallenge(gateway: Self.gateway)
        let assertion = try await DCAppAttestService.shared.generateAssertion(
            keyId, clientDataHash: pushDelegateHash(challenge: c, delegation: d))
        do {
            state.grant = try await pushDelegate(gateway: Self.gateway, challenge: c, delegation: d,
                                                 assertion: assertion)
        } catch BuzzError.Gone(_) where again {
            state.installation = nil
            try await delegate(to: r, endpoint: endpoint, again: false)
            return
        }
        state.grantExpires = d.expiresAt
        state.relayPubkey = r.pubkey
        // A delegation extends the installation to its own expiry.
        state.installationExpires = max(state.installationExpires, d.expiresAt)
        state.save()
    }

    /// A new App Attest key, attested over the enrollment of `endpoint`.
    @MainActor
    private func enroll(_ endpoint: String) async throws {
        let service = DCAppAttestService.shared
        let keyId = try await service.generateKey()
        let expires = Self.now() + Self.lifetime
        let c = try await pushChallenge(gateway: Self.gateway)
        let attestation = try await service.attestKey(
            keyId, clientDataHash: pushEnrollHash(challenge: c, keyId: keyId, endpoint: endpoint, expiresAt: expires))
        let installation = try await pushEnroll(gateway: Self.gateway, challenge: c, keyId: keyId,
                                                attestation: attestation, endpoint: endpoint, expiresAt: expires)
        state.keyId = keyId
        state.installation = installation
        state.installationExpires = expires
        state.endpoint = endpoint
        state.generation = 0
        state.grant = nil
        state.save()
    }
}

/// Hands the APNs token to `Push`.
final class AppDelegate: NSObject, UIApplicationDelegate {
    func application(_ application: UIApplication,
                     didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data) {
        Push.shared.registered(token: deviceToken)
    }

    func application(_ application: UIApplication,
                     didFailToRegisterForRemoteNotificationsWithError error: Error) {
        NSLog("push: no APNs token: \(error)")
    }
}
