@preconcurrency import Foundation
@preconcurrency import NetworkExtension
import MyproxyNetworkShared

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
        try Task.checkCancellation()
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
        try Task.checkCancellation()
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
        (try? await connectionStatus()) ?? false
    }

    func connectionStatus() async throws -> Bool {
        guard let loaded = try await loadOwnedManager() else { return false }
        try await load(loaded)
        manager = loaded
        return loaded.connection.status == .connected
    }

    func configureAndApplyRunning(
        _ configuration: [String: NSObject],
        revision: UInt64
    ) async throws {
        try await configure(configuration)
        try Task.checkCancellation()
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

        try Task.checkCancellation()
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

    func runtimeStatus() async throws -> HostProviderControlResponse {
        try await send(HostProviderControlRequest(
            command: "dnsStatus", revision: nil, activationIdentifier: nil,
            dnsProxyBootstrap: nil, captureEnabled: nil, failOpen: nil,
            captureConfigurationSnapshot: nil, mihomoRouteProxyCatalog: nil,
            mihomoSOCKSHost: nil, mihomoSOCKSPort: nil,
            mihomoSOCKSUsername: nil, mihomoSOCKSPassword: nil
        ))
    }

    func fetchActivity(cursor: UInt64, limit: Int) async throws -> HostProviderControlResponse {
        try await send(HostProviderControlRequest(
            command: "activity",
            revision: nil,
            activationIdentifier: nil,
            dnsProxyBootstrap: nil,
            captureEnabled: nil,
            failOpen: nil,
            captureConfigurationSnapshot: nil,
            mihomoRouteProxyCatalog: nil,
            mihomoSOCKSHost: nil,
            mihomoSOCKSPort: nil,
            mihomoSOCKSUsername: nil,
            mihomoSOCKSPassword: nil,
            activityCursor: cursor,
            activityLimit: limit
        ))
    }

    func stop() async throws {
        let loadedManager: NETransparentProxyManager?
        if let manager {
            loadedManager = manager
        } else {
            loadedManager = try await loadOwnedManager()
        }
        guard let manager = loadedManager else { return }
        self.manager = manager
        // Tear the tunnel down before preference I/O so a hung nehelper
        // callback cannot leave capture running.
        switch manager.connection.status {
        case .disconnected, .invalid:
            break
        default:
            manager.connection.stopVPNTunnel()
            do {
                try await waitForConnection(manager.connection, target: .disconnected)
            } catch let failure as NetworkExtensionControlFailure {
                // A session wedged in disconnecting survives stopVPNTunnel, and
                // every later enable() starts with this stop. Drop the saved
                // configuration instead of failing the whole enable.
                AppLog.warn(
                    "ne-host",
                    "transparent proxy did not stop (\(failure.message)); dropping the configuration"
                )
                try await reset(manager)
                return
            }
        }
        try Task.checkCancellation()
        try await load(manager)
        try Task.checkCancellation()
        manager.isEnabled = false
        try await save(manager)
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

    /// Delete a configuration whose session cannot be driven down, so the next
    /// `configure` builds a new manager from preferences instead of reusing the
    /// wedged one.
    private func reset(_ manager: NETransparentProxyManager) async throws {
        try await remove(manager)
        self.manager = nil
    }

    private func remove(_ manager: NETransparentProxyManager) async throws {
        try await awaitPreferenceCallback(
            operation: .stopTransparentProxy,
            api: "removeFromPreferences"
        ) { completion in
            manager.removeFromPreferences(completionHandler: completion)
        }
    }

    private func loadOwnedManager() async throws -> NETransparentProxyManager? {
        let loaded: LoadedManagers = try await awaitOnceReply(
            timeout: OnceReplyTimeout.preferences,
            operation: .configureTransparentProxy,
            timeoutMessage: "Timed out waiting for loadAllFromPreferences"
        ) { reply in
            NETransparentProxyManager.loadAllFromPreferences { managers, error in
                if let error {
                    reply.finish(.failure(error))
                } else {
                    reply.finish(.success(LoadedManagers(values: managers ?? [])))
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
        try await awaitPreferenceCallback(
            operation: .configureTransparentProxy,
            api: "saveToPreferences"
        ) { completion in
            manager.saveToPreferences(completionHandler: completion)
        }
    }

    private func load(_ manager: NETransparentProxyManager) async throws {
        try await awaitPreferenceCallback(
            operation: .configureTransparentProxy,
            api: "loadFromPreferences"
        ) { completion in
            manager.loadFromPreferences(completionHandler: completion)
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
            try Task.checkCancellation()
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
        try Task.checkCancellation()
        let payload = try JSONEncoder().encode(request)
        let responseData: Data = try await awaitOnceReply(
            timeout: OnceReplyTimeout.providerMessage,
            operation: .configureTransparentProxy,
            timeoutMessage: "Timed out waiting for provider status"
        ) { reply in
            do {
                try session.sendProviderMessage(payload) { response in
                    if let response {
                        reply.finish(.success(response))
                    } else {
                        reply.finish(
                            .failure(
                                NetworkExtensionControlFailure(
                                    operation: .configureTransparentProxy,
                                    message: "Provider returned an empty control response"
                                )
                            )
                        )
                    }
                }
            } catch {
                reply.finish(
                    .failure(
                        NetworkExtensionControlFailure(
                            operation: .configureTransparentProxy,
                            underlying: error
                        )
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
    var activityCursor: UInt64? = nil
    var activityLimit: Int? = nil
}

struct HostProviderControlResponse: Decodable, Sendable {
    let accepted: Bool
    let revision: UInt64
    let running: Bool
    let captureEnabled: Bool
    let failOpen: Bool?
    let message: String?
    let activityBatch: AppRoutingActivityBatch?
    let dnsRuntimeReport: DNSProxyRuntimeReport?
}
