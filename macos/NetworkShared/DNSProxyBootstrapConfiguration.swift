import Foundation

/// One versioned, property-list-safe payload used to start the DNS provider.
///
/// `NEDNSProxyProviderProtocol.providerConfiguration` crosses a Foundation/XPC
/// boundary. Keeping the runtime identity and private relay endpoint inside a
/// single `Data` value avoids independently bridging heterogeneous NSNumber,
/// NSString, and NSData fields in the provider entry point.
public struct DNSProxyBootstrapConfiguration: Codable, Equatable, Sendable {
    public static let currentSchemaVersion = 3
    // The capture snapshot itself may be 8 MiB. JSON represents Data as
    // base64, so the atomic DNS bootstrap needs headroom for that expansion,
    // the route catalog, and schema metadata.
    public static let maximumEncodedSize = 12 * 1_024 * 1_024
    public static let maximumUpstreamResolvers = 8

    public let schemaVersion: Int
    public let revision: UInt64
    public let activationIdentifier: UUID
    public let profileRulesProxy: MihomoRouteProxyEndpoint
    public let routeProxyEndpoints: [MihomoRouteProxyEndpoint]?
    /// Resolvers for queries macOS hands over as a *name* endpoint. The system
    /// resolver path reports the queried name instead of the resolver address,
    /// so those flows can only be answered by an upstream the provider knows.
    public let upstreamResolvers: [String]?
    public let encodedCaptureSnapshot: Data?

    public init(
        revision: UInt64,
        activationIdentifier: UUID,
        profileRulesProxy: MihomoRouteProxyEndpoint,
        routeProxyEndpoints: [MihomoRouteProxyEndpoint]? = nil,
        upstreamResolvers: [String]? = nil,
        encodedCaptureSnapshot: Data? = nil
    ) throws {
        schemaVersion = Self.currentSchemaVersion
        self.revision = revision
        self.activationIdentifier = activationIdentifier
        self.profileRulesProxy = profileRulesProxy
        self.routeProxyEndpoints = routeProxyEndpoints
        self.upstreamResolvers = upstreamResolvers
        self.encodedCaptureSnapshot = encodedCaptureSnapshot
        try validate()
    }

    public func validate() throws {
        // Schema 1 and 2 bootstrap payloads carry no resolver list; they stay
        // accepted so an extension update can read a configuration written by an
        // older host.
        let supportedSchemas = [1, 2, Self.currentSchemaVersion]
        guard supportedSchemas.contains(schemaVersion) else {
            throw DNSProxyBootstrapConfigurationError.unsupportedSchemaVersion(
                schemaVersion
            )
        }
        guard revision > 0 else {
            throw DNSProxyBootstrapConfigurationError.invalidRevision(revision)
        }
        guard profileRulesProxy.route == .profileRules else {
            throw DNSProxyBootstrapConfigurationError.invalidProfileRulesRoute
        }
        if let upstreamResolvers {
            guard upstreamResolvers.count <= Self.maximumUpstreamResolvers,
                  upstreamResolvers.allSatisfy(DNSProxyUpstreamResolver.isValid)
            else {
                throw DNSProxyBootstrapConfigurationError.invalidUpstreamResolvers
            }
        }
        do {
            try MihomoRouteProxyCatalog.validate(
                routeProxyEndpoints ?? [profileRulesProxy]
            )
        } catch {
            throw DNSProxyBootstrapConfigurationError.invalidProfileRulesProxy
        }
    }

    public func encoded() throws -> Data {
        try validate()
        let data = try JSONEncoder().encode(self)
        guard data.count <= Self.maximumEncodedSize else {
            throw DNSProxyBootstrapConfigurationError.encodedPayloadTooLarge(
                actual: data.count,
                maximum: Self.maximumEncodedSize
            )
        }
        return data
    }

    public static func decode(_ data: Data) throws -> Self {
        guard data.count <= maximumEncodedSize else {
            throw DNSProxyBootstrapConfigurationError.encodedPayloadTooLarge(
                actual: data.count,
                maximum: maximumEncodedSize
            )
        }
        let value = try JSONDecoder().decode(Self.self, from: data)
        try value.validate()
        return value
    }

    /// Selects one activation bootstrap atomically at the provider boundary.
    /// A host-staged value is authoritative; framework-delivered options may
    /// confirm it but may never replace a different activation.
    public static func resolve(
        prepared: Self?,
        delivered: Self?
    ) throws -> Self {
        switch (prepared, delivered) {
        case let (prepared?, delivered?) where prepared == delivered:
            return prepared
        case (.some, .some):
            throw DNSProxyBootstrapResolutionError.deliveredBootstrapMismatch
        case let (prepared?, nil):
            return prepared
        case let (nil, delivered?):
            return delivered
        case (nil, nil):
            throw DNSProxyBootstrapResolutionError.bootstrapUnavailable
        }
    }
}

public enum DNSProxyBootstrapResolutionError: Error, Equatable, Sendable {
    case bootstrapUnavailable
    case deliveredBootstrapMismatch
}

public enum DNSProxyBootstrapConfigurationError: Error, Equatable, Sendable {
    case unsupportedSchemaVersion(Int)
    case invalidRevision(UInt64)
    case invalidProfileRulesRoute
    case invalidProfileRulesProxy
    case invalidUpstreamResolvers
    case encodedPayloadTooLarge(actual: Int, maximum: Int)
}

extension DNSProxyBootstrapConfigurationError: LocalizedError {
    public var errorDescription: String? {
        switch self {
        case let .unsupportedSchemaVersion(version):
            "DNS proxy bootstrap uses unsupported schema version \(version)."
        case let .invalidRevision(revision):
            "DNS proxy bootstrap revision must be greater than zero; received \(revision)."
        case .invalidProfileRulesRoute:
            "DNS proxy bootstrap must use the profile-rules Mihomo route."
        case .invalidProfileRulesProxy:
            "DNS proxy bootstrap contains an invalid private Mihomo SOCKS5 endpoint."
        case .invalidUpstreamResolvers:
            "DNS proxy bootstrap contains invalid upstream resolvers."
        case let .encodedPayloadTooLarge(actual, maximum):
            "DNS proxy bootstrap is \(actual) bytes; the maximum is \(maximum)."
        }
    }
}

/// One upstream resolver the DNS provider relays name-endpoint queries to.
public enum DNSProxyUpstreamResolver {
    public static let defaultPort: UInt16 = 53

    public static func isValid(_ spec: String) -> Bool {
        endpoint(for: spec) != nil
    }

    /// Parses `1.1.1.1`, `1.1.1.1:53`, `[2606:4700:4700::1111]:53`, or a bare
    /// IPv6 address. Hostnames are rejected: a name endpoint must not become
    /// another name lookup the provider cannot complete.
    public static func endpoint(for spec: String) -> SOCKS5Endpoint? {
        let trimmed = spec.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        var host = trimmed
        var port = defaultPort
        if trimmed.hasPrefix("[") {
            guard let closing = trimmed.firstIndex(of: "]") else { return nil }
            host = String(trimmed[trimmed.index(after: trimmed.startIndex) ..< closing])
            let remainder = trimmed[trimmed.index(after: closing)...]
            if !remainder.isEmpty {
                guard remainder.hasPrefix(":"),
                      let parsed = UInt16(remainder.dropFirst())
                else { return nil }
                port = parsed
            }
        } else if trimmed.filter({ $0 == ":" }).count == 1,
                  let separator = trimmed.firstIndex(of: ":") {
            host = String(trimmed[..<separator])
            guard let parsed = UInt16(trimmed[trimmed.index(after: separator)...]) else {
                return nil
            }
            port = parsed
        }
        guard port > 0, let address = try? IPAddress(host) else { return nil }
        return SOCKS5Endpoint(address: SOCKS5Address(ipAddress: address), port: port)
    }
}
