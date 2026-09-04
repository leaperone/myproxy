@preconcurrency import Foundation
@preconcurrency import NetworkExtension

actor AppleTransparentProxyManager {
    private struct LoadedManagers: @unchecked Sendable {
        let values: [NETransparentProxyManager]
    }

    private let providerBundleIdentifier: String
    private let connectionTimeout: Duration
    private var manager: NETransparentProxyManager?

    init(
        providerBundleIdentifier: String = MyproxyNetworkExtensionIdentifiers.systemExtension,
        connectionTimeout: Duration = .seconds(20)
    ) {
        self.providerBundleIdentifier = providerBundleIdentifier
        self.connectionTimeout = connectionTimeout
    }

    func configure(_ configuration: [String: NSObject]) async throws {
        let manager = try await loadOwnedManager() ?? NETransparentProxyManager()
        let providerProtocol = NETunnelProviderProtocol()
        providerProtocol.providerBundleIdentifier = providerBundleIdentifier
        providerProtocol.serverAddress = "myproxy Local Transparent Proxy"
        providerProtocol.providerConfiguration = configuration
        manager.protocolConfiguration = providerProtocol
        manager.localizedDescription = MyproxyNetworkExtensionIdentifiers.localizedDescription
        manager.isEnabled = true
        try await save(manager)
        try await load(manager)
        self.manager = manager
    }

    func start() async throws {
        try await reload()
        guard let manager, manager.isEnabled else {
            throw NetworkExtensionControlFailure(
                operation: .startTransparentProxy,
                message: "Transparent proxy configuration is disabled"
            )
        }
        do {
            try manager.connection.startVPNTunnel()
        } catch {
            throw NetworkExtensionControlFailure(
                operation: .startTransparentProxy,
                underlying: error
            )
        }
        try await waitForConnection(manager.connection, target: .connected)
    }

    func stop() async throws {
        let loadedManager: NETransparentProxyManager?
        if let manager {
            loadedManager = manager
        } else {
            loadedManager = try await loadOwnedManager()
        }
        guard let manager = loadedManager else { return }
        try await load(manager)
        self.manager = manager
        switch manager.connection.status {
        case .disconnected, .invalid:
            return
        default:
            manager.connection.stopVPNTunnel()
            try await waitForConnection(manager.connection, target: .disconnected)
        }
    }

    private func reload() async throws {
        let loadedManager: NETransparentProxyManager?
        if let manager {
            loadedManager = manager
        } else {
            loadedManager = try await loadOwnedManager()
        }
        guard let manager = loadedManager else {
            throw NetworkExtensionControlFailure(
                operation: .configureTransparentProxy,
                message: "No myproxy transparent proxy configuration exists"
            )
        }
        try await load(manager)
        self.manager = manager
    }

    private func loadOwnedManager() async throws -> NETransparentProxyManager? {
        let loaded: LoadedManagers = try await withCheckedThrowingContinuation { continuation in
            NETransparentProxyManager.loadAllFromPreferences { managers, error in
                if let error {
                    continuation.resume(throwing: error)
                } else {
                    continuation.resume(returning: LoadedManagers(values: managers ?? []))
                }
            }
        }
        return loaded.values.first { manager in
            guard let providerProtocol = manager.protocolConfiguration
                as? NETunnelProviderProtocol
            else {
                return false
            }
            return providerProtocol.providerBundleIdentifier == providerBundleIdentifier
        }
    }

    private func save(_ manager: NETransparentProxyManager) async throws {
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

    private func load(_ manager: NETransparentProxyManager) async throws {
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

    private func waitForConnection(
        _ connection: NEVPNConnection,
        target: NEVPNStatus
    ) async throws {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: connectionTimeout)
        var observedConnectionAttempt = connection.status != .disconnected
        while clock.now < deadline {
            let status = connection.status
            if status == target { return }
            if target == .connected {
                switch status {
                case .connecting, .connected, .reasserting, .disconnecting:
                    observedConnectionAttempt = true
                case .invalid:
                    throw NetworkExtensionControlFailure(
                        operation: .startTransparentProxy,
                        message: "Transparent proxy connection became invalid"
                    )
                case .disconnected where observedConnectionAttempt:
                    throw NetworkExtensionControlFailure(
                        operation: .startTransparentProxy,
                        message: "Transparent proxy disconnected during startup"
                    )
                default:
                    break
                }
            }
            try await Task.sleep(for: .milliseconds(100))
        }
        throw NetworkExtensionControlFailure(
            operation: target == .connected ? .startTransparentProxy : .stopTransparentProxy,
            message: "Timed out waiting for transparent proxy status \(target.rawValue)"
        )
    }
}
