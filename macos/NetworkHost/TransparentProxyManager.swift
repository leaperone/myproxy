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

    func isConnected() async -> Bool {
        do {
            try await reload()
            return manager?.connection.status == .connected
        } catch {
            return false
        }
    }

    func configureAndApplyRunning(
        _ configuration: [String: NSObject],
        revision: UInt64
    ) async throws {
        try await configure(configuration)
        try await applyRunningConfiguration(configuration, revision: revision)
    }

    func applyRunningConfiguration(
        _ configuration: [String: NSObject],
        revision: UInt64
    ) async throws {
        let quiesced = try await send(
            HostProviderControlRequest(
                command: "quiesce",
                revision: revision,
                activationIdentifier: nil,
                dnsProxyBootstrap: nil,
                captureEnabled: false,
                failOpen: true,
                captureConfigurationSnapshot: nil,
                mihomoRouteProxyCatalog: nil,
                mihomoSOCKSHost: nil,
                mihomoSOCKSPort: nil,
                mihomoSOCKSUsername: nil,
                mihomoSOCKSPassword: nil
            )
        )
        guard quiesced.accepted, quiesced.revision == revision else {
            throw NetworkExtensionControlFailure(
                operation: .configureTransparentProxy,
                message: quiesced.message ?? "Provider refused to quiesce for a live update"
            )
        }

        let applied = try await send(
            HostProviderControlRequest(
                command: "applyConfiguration",
                revision: revision,
                activationIdentifier: uuid(configuration["activationIdentifier"]),
                dnsProxyBootstrap: data(configuration["dnsProxyBootstrap"]),
                captureEnabled: true,
                failOpen: true,
                captureConfigurationSnapshot: data(
                    configuration["captureConfigurationSnapshot"]
                ),
                mihomoRouteProxyCatalog: data(
                    configuration["mihomoRouteProxyCatalog"]
                ),
                mihomoSOCKSHost: configuration["mihomoSOCKSHost"] as? String,
                mihomoSOCKSPort: uint16(configuration["mihomoSOCKSPort"]),
                mihomoSOCKSUsername: configuration["mihomoSOCKSUsername"] as? String,
                mihomoSOCKSPassword: configuration["mihomoSOCKSPassword"] as? String
            )
        )
        guard applied.accepted, applied.revision == revision, applied.captureEnabled else {
            throw NetworkExtensionControlFailure(
                operation: .configureTransparentProxy,
                message: applied.message ?? "Provider refused the live capture snapshot"
            )
        }
    }

    func prepareDNS(
        revision: UInt64,
        activationIdentifier: UUID,
        bootstrap: Data
    ) async throws {
        let response = try await send(
            HostProviderControlRequest(
                command: "prepareDNS",
                revision: revision,
                activationIdentifier: activationIdentifier,
                dnsProxyBootstrap: bootstrap,
                captureEnabled: nil,
                failOpen: nil,
                captureConfigurationSnapshot: nil,
                mihomoRouteProxyCatalog: nil,
                mihomoSOCKSHost: nil,
                mihomoSOCKSPort: nil,
                mihomoSOCKSUsername: nil,
                mihomoSOCKSPassword: nil
            )
        )
        guard response.accepted, response.revision == revision else {
            throw NetworkExtensionControlFailure(
                operation: .configureDNSProxy,
                message: response.message ?? "Transparent provider refused DNS preparation"
            )
        }
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

    private func send(
        _ request: HostProviderControlRequest
    ) async throws -> HostProviderControlResponse {
        try await reload()
        guard let manager else {
            throw NetworkExtensionControlFailure(
                operation: .configureTransparentProxy,
                message: "No myproxy transparent proxy configuration exists"
            )
        }
        guard let session = manager.connection as? NETunnelProviderSession else {
            throw NetworkExtensionControlFailure(
                operation: .configureTransparentProxy,
                message: "Transparent proxy session is not available"
            )
        }
        let payload = try JSONEncoder().encode(request)
        let responseData: Data = try await withCheckedThrowingContinuation { continuation in
            do {
                try session.sendProviderMessage(payload) { response in
                    guard let response else {
                        continuation.resume(
                            throwing: NetworkExtensionControlFailure(
                                operation: .configureTransparentProxy,
                                message: "Provider returned an empty control response"
                            )
                        )
                        return
                    }
                    continuation.resume(returning: response)
                }
            } catch {
                continuation.resume(
                    throwing: NetworkExtensionControlFailure(
                        operation: .configureTransparentProxy,
                        underlying: error
                    )
                )
            }
        }
        do {
            return try JSONDecoder().decode(
                HostProviderControlResponse.self,
                from: responseData
            )
        } catch {
            throw NetworkExtensionControlFailure(
                operation: .configureTransparentProxy,
                message: "Provider returned an invalid control response"
            )
        }
    }

    private func data(_ value: Any?) -> Data? {
        switch value {
        case let value as Data:
            value
        case let value as NSData:
            value as Data
        default:
            nil
        }
    }

    private func uint16(_ value: Any?) -> UInt16? {
        switch value {
        case let value as UInt16 where value > 0:
            value
        case let value as Int where (1 ... Int(UInt16.max)).contains(value):
            UInt16(value)
        case let value as NSNumber where (1 ... Int(UInt16.max)).contains(value.intValue):
            UInt16(value.intValue)
        default:
            nil
        }
    }

    private func uuid(_ value: Any?) -> UUID? {
        switch value {
        case let value as UUID:
            value
        case let value as String:
            UUID(uuidString: value)
        case let value as NSString:
            UUID(uuidString: value as String)
        default:
            nil
        }
    }
}

private struct HostProviderControlRequest: Encodable, Sendable {
    let protocolVersion = 3
    let command: String
    let revision: UInt64?
    let activationIdentifier: UUID?
    let dnsProxyBootstrap: Data?
    let captureEnabled: Bool?
    let failOpen: Bool?
    let captureConfigurationSnapshot: Data?
    let mihomoRouteProxyCatalog: Data?
    let mihomoSOCKSHost: String?
    let mihomoSOCKSPort: UInt16?
    let mihomoSOCKSUsername: String?
    let mihomoSOCKSPassword: String?
}

private struct HostProviderControlResponse: Decodable, Sendable {
    let accepted: Bool
    let revision: UInt64
    let running: Bool
    let captureEnabled: Bool
    let message: String?
}
