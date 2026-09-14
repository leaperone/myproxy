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
        try await load(manager, operation: .configureDNSProxy)
        try Task.checkCancellation()
        let providerProtocol = NEDNSProxyProviderProtocol()
        providerProtocol.providerBundleIdentifier = providerBundleIdentifier
        providerProtocol.providerConfiguration = [
            "dnsProxyBootstrap": bootstrap
        ]
        manager.providerProtocol = providerProtocol
        manager.localizedDescription = MyproxyNetworkExtensionIdentifiers.localizedDescription
        manager.isEnabled = true
        do {
            try await save(manager, operation: .configureDNSProxy)
        } catch {
            manager.isEnabled = false
            if !isPreferenceTimeout(error) {
                try? await save(manager, operation: .configureDNSProxy)
            }
            throw NetworkExtensionControlFailure(
                operation: .configureDNSProxy,
                underlying: error
            )
        }
        self.manager = manager
    }

    func disable() async throws {
        let manager = NEDNSProxyManager.shared()
        try await load(manager, operation: .stopDNSProxy)
        try Task.checkCancellation()
        guard manager.providerProtocol?.providerBundleIdentifier == providerBundleIdentifier else {
            self.manager = nil
            return
        }
        do {
            manager.isEnabled = false
            try await save(manager, operation: .stopDNSProxy)
            try await remove(manager, operation: .stopDNSProxy)
        } catch {
            // Saving disabled is the important resolver restoration step;
            // remove is best effort because macOS may reject it while the
            // provider is stopping. Skip another round-trip when nehelper
            // already timed out.
            if !isPreferenceTimeout(error) {
                manager.isEnabled = false
                try? await save(manager, operation: .stopDNSProxy)
            }
            self.manager = manager
            throw NetworkExtensionControlFailure(
                operation: .stopDNSProxy,
                underlying: error
            )
        }
        self.manager = nil
    }

    private func load(
        _ manager: NEDNSProxyManager,
        operation: NetworkExtensionControlOperation
    ) async throws {
        try await awaitPreferenceCallback(
            operation: operation,
            api: "loadFromPreferences"
        ) { completion in
            manager.loadFromPreferences(completionHandler: completion)
        }
    }

    private func save(
        _ manager: NEDNSProxyManager,
        operation: NetworkExtensionControlOperation
    ) async throws {
        try await awaitPreferenceCallback(
            operation: operation,
            api: "saveToPreferences"
        ) { completion in
            manager.saveToPreferences(completionHandler: completion)
        }
    }

    private func remove(
        _ manager: NEDNSProxyManager,
        operation: NetworkExtensionControlOperation
    ) async throws {
        try await awaitPreferenceCallback(
            operation: operation,
            api: "removeFromPreferences"
        ) { completion in
            manager.removeFromPreferences(completionHandler: completion)
        }
    }
}

private func isPreferenceTimeout(_ error: Error) -> Bool {
    guard let failure = error as? NetworkExtensionControlFailure else { return false }
    return failure.message.hasPrefix("Timed out waiting for ")
}
