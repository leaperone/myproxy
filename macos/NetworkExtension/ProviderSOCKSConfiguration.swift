import Foundation
import MyproxyNetworkShared
@preconcurrency import Network

struct ProviderSOCKSConfiguration: Equatable, Sendable {
    let host: String
    let port: UInt16
    let credentials: SOCKS5UsernamePasswordCredentials?

    init?(providerConfiguration: [String: Any]?) {
        guard let providerConfiguration,
              let host = providerConfiguration[ProviderConfigurationKey.mihomoSOCKSHost]
                as? String,
              host == "127.0.0.1" || host == "::1",
              let port = Self.uint16(
                providerConfiguration[ProviderConfigurationKey.mihomoSOCKSPort]
              )
        else {
            return nil
        }

        let username = providerConfiguration[
            ProviderConfigurationKey.mihomoSOCKSUsername
        ] as? String
        let password = providerConfiguration[
            ProviderConfigurationKey.mihomoSOCKSPassword
        ] as? String
        let credentials: SOCKS5UsernamePasswordCredentials?
        switch (username, password) {
        case (nil, nil):
            credentials = nil
        case let (username?, password?):
            guard let value = try? SOCKS5UsernamePasswordCredentials(
                username: username,
                password: password
            ) else {
                return nil
            }
            credentials = value
        default:
            return nil
        }

        self.host = host
        self.port = port
        self.credentials = credentials
    }

    init?(routeEndpoint: MihomoRouteProxyEndpoint) {
        guard routeEndpoint.host == "127.0.0.1" || routeEndpoint.host == "::1",
              routeEndpoint.port > 0
        else { return nil }
        let credentials: SOCKS5UsernamePasswordCredentials?
        switch (routeEndpoint.username, routeEndpoint.password) {
        case (nil, nil):
            credentials = nil
        case let (username?, password?):
            guard let value = try? SOCKS5UsernamePasswordCredentials(
                username: username,
                password: password
            ) else { return nil }
            credentials = value
        default:
            return nil
        }
        host = routeEndpoint.host
        port = routeEndpoint.port
        self.credentials = credentials
    }

    static func routeCatalog(
        providerConfiguration: [String: Any]?
    ) -> [MihomoRoute: ProviderSOCKSConfiguration]? {
        guard let providerConfiguration else { return nil }
        if let data = providerConfiguration[
            ProviderConfigurationKey.mihomoRouteProxyCatalog
        ] as? Data {
            guard let endpoints = try? MihomoRouteProxyCatalog.decode(data) else {
                return nil
            }
            var result: [MihomoRoute: ProviderSOCKSConfiguration] = [:]
            for endpoint in endpoints {
                guard let configuration = ProviderSOCKSConfiguration(
                    routeEndpoint: endpoint
                ) else { return nil }
                result[endpoint.route] = configuration
            }
            return result[.profileRules] == nil ? nil : result
        }
        guard let profileRules = ProviderSOCKSConfiguration(
            providerConfiguration: providerConfiguration
        ) else { return nil }
        return [.profileRules: profileRules]
    }

    static func proxy(
        for route: MihomoRoute,
        in routeCatalog: [MihomoRoute: ProviderSOCKSConfiguration]
    ) -> ProviderSOCKSConfiguration? {
        routeCatalog[route]
    }

    /// Builds the route-specific portion shared by TCP and UDP Mihomo
    /// interception plans. A decision receives a proxy only when its complete
    /// route target exists in the catalog; no profile or legacy-route aliasing
    /// is attempted.
    static func flowPlan(
        for decision: FlowTrafficDecision,
        endpoint: FlowRemoteEndpoint,
        preferredHostname: String?,
        routeCatalog: [MihomoRoute: ProviderSOCKSConfiguration]
    ) throws -> ProviderSOCKSFlowPlan? {
        switch decision.disposition {
        case .direct, .reject, .failOpen:
            return nil
        case let .mihomo(route):
            return ProviderSOCKSFlowPlan(
                destinations: try destinations(
                    for: endpoint,
                    preferredHostname: preferredHostname
                ),
                proxy: proxy(for: route, in: routeCatalog)
            )
        }
    }

    var networkHost: NWEndpoint.Host { NWEndpoint.Host(host) }

    var networkPort: NWEndpoint.Port {
        // `port` is already non-zero and UInt16-bounded by construction.
        NWEndpoint.Port(rawValue: port)!
    }

    func destination(for endpoint: FlowRemoteEndpoint) throws -> SOCKS5Endpoint {
        try Self.destination(for: endpoint)
    }

    static func destination(for endpoint: FlowRemoteEndpoint) throws -> SOCKS5Endpoint {
        guard let port = UInt16(endpoint.port), port > 0 else {
            throw FlowContextConversionFailure.invalidRemotePort(endpoint.port)
        }
        return SOCKS5Endpoint(
            address: try socksAddress(for: endpoint.host),
            port: port
        )
    }

    /// Builds the two destinations needed by one interception plan. Direct
    /// relay and fail-open recovery retain the endpoint supplied by macOS,
    /// while Mihomo receives the hostname that was validated during rule
    /// evaluation whenever SOCKS5 can represent it safely.
    static func destinations(
        for endpoint: FlowRemoteEndpoint,
        preferredHostname: String?
    ) throws -> ProviderSOCKSDestinations {
        let original = try destination(for: endpoint)
        guard let preferredHostname,
              let preferredAddress = try? socksAddress(for: preferredHostname)
        else {
            return ProviderSOCKSDestinations(
                original: original,
                mihomo: original
            )
        }
        return ProviderSOCKSDestinations(
            original: original,
            mihomo: SOCKS5Endpoint(
                address: preferredAddress,
                port: original.port
            )
        )
    }

    private static func socksAddress(for value: String) throws -> SOCKS5Address {
        var host = value.trimmingCharacters(in: .whitespacesAndNewlines)
        if host.hasPrefix("["), host.hasSuffix("]") {
            host.removeFirst()
            host.removeLast()
        }
        if let address = try? IPAddress(host) {
            return SOCKS5Address(ipAddress: address)
        }
        return try SOCKS5Address(domain: host)
    }

    private static func uint16(_ value: Any?) -> UInt16? {
        switch value {
        case let value as UInt16 where value > 0:
            value
        case let value as Int where (1 ... Int(UInt16.max)).contains(value):
            UInt16(value)
        case let value as NSNumber where (1 ... Int(UInt16.max)).contains(value.intValue):
            UInt16(value.intValue)
        case let value as String:
            UInt16(value).flatMap { $0 > 0 ? $0 : nil }
        default:
            nil
        }
    }
}

struct ProviderSOCKSDestinations: Equatable, Sendable {
    let original: SOCKS5Endpoint
    let mihomo: SOCKS5Endpoint
}

struct ProviderSOCKSFlowPlan: Equatable, Sendable {
    let destinations: ProviderSOCKSDestinations
    let proxy: ProviderSOCKSConfiguration?
}
