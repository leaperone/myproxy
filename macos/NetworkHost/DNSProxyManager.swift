@preconcurrency import Foundation
@preconcurrency import NetworkExtension

/// Owns myproxy's system DNS proxy configuration. NEDNSProxyManager has no
/// start method: saving an enabled provider configuration activates it, and
/// removing that configuration restores the resolver managed by macOS.
actor AppleDNSProxyManager {
    private let providerBundleIdentifier: String
    private var manager: NEDNSProxyManager?

    init(
        providerBundleIdentifier: String = MyproxyNetworkExtensionIdentifiers.systemExtension
    ) {
        self.providerBundleIdentifier = providerBundleIdentifier
    }

    func configureAndEnable(_ bootstrap: Data) async throws {
        let manager = NEDNSProxyManager.shared()
        try await load(manager)
        let providerProtocol = NEDNSProxyProviderProtocol()
        providerProtocol.providerBundleIdentifier = providerBundleIdentifier
        providerProtocol.providerConfiguration = [
            "dnsProxyBootstrap": bootstrap
        ]
        manager.providerProtocol = providerProtocol
        manager.localizedDescription = MyproxyNetworkExtensionIdentifiers.localizedDescription
        manager.isEnabled = true
        do {
            try await save(manager)
        } catch {
            manager.isEnabled = false
            try? await save(manager)
            throw NetworkExtensionControlFailure(
                operation: .configureDNSProxy,
                underlying: error
            )
        }
        self.manager = manager
    }

    func disable() async throws {
        let manager = NEDNSProxyManager.shared()
        try await load(manager)
        guard manager.providerProtocol?.providerBundleIdentifier == providerBundleIdentifier else {
            self.manager = nil
            return
        }
        do {
            manager.isEnabled = false
            try await save(manager)
            try await remove(manager)
        } catch {
            // Saving disabled is the important resolver restoration step;
            // remove is best effort because macOS may reject it while the
            // provider is stopping.
            manager.isEnabled = false
            try? await save(manager)
            self.manager = manager
            throw NetworkExtensionControlFailure(
                operation: .stopDNSProxy,
                underlying: error
            )
        }
        self.manager = nil
    }

    private func load(_ manager: NEDNSProxyManager) async throws {
        try await withCheckedThrowingContinuation {
            (continuation: CheckedContinuation<Void, Error>) in
            manager.loadFromPreferences { error in
                if let error {
                    continuation.resume(throwing: error)
                } else {
                    continuation.resume(returning: ())
                }
            }
        }
    }

    private func save(_ manager: NEDNSProxyManager) async throws {
        try await withCheckedThrowingContinuation {
            (continuation: CheckedContinuation<Void, Error>) in
            manager.saveToPreferences { error in
                if let error {
                    continuation.resume(throwing: error)
                } else {
                    continuation.resume(returning: ())
                }
            }
        }
    }

    private func remove(_ manager: NEDNSProxyManager) async throws {
        try await withCheckedThrowingContinuation {
            (continuation: CheckedContinuation<Void, Error>) in
            manager.removeFromPreferences { error in
                if let error {
                    continuation.resume(throwing: error)
                } else {
                    continuation.resume(returning: ())
                }
            }
        }
    }
}
