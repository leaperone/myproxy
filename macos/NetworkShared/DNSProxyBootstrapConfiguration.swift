import Foundation

/// One versioned, property-list-safe payload used to start the DNS provider.
///
/// `NEDNSProxyProviderProtocol.providerConfiguration` crosses a Foundation/XPC
/// boundary. Keeping the runtime identity and private relay endpoint inside a
/// single `Data` value avoids independently bridging heterogeneous NSNumber,
/// NSString, and NSData fields in the provider entry point.
public struct DNSProxyBootstrapConfiguration: Codable, Equatable, Sendable {
    public static let currentSchemaVersion = 2
    // The capture snapshot itself may be 8 MiB. JSON represents Data as
    // base64, so the atomic DNS bootstrap needs headroom for that expansion,
    // the route catalog, and schema metadata.
    public static let maximumEncodedSize = 12 * 1_024 * 1_024

    public let schemaVersion: Int
    public let revision: UInt64
    public let activationIdentifier: UUID
    public let profileRulesProxy: MihomoRouteProxyEndpoint
    public let routeProxyEndpoints: [MihomoRouteProxyEndpoint]?
    public let encodedCaptureSnapshot: Data?

    public init(
        revision: UInt64,
        activationIdentifier: UUID,
        profileRulesProxy: MihomoRouteProxyEndpoint,
        routeProxyEndpoints: [MihomoRouteProxyEndpoint]? = nil,
        encodedCaptureSnapshot: Data? = nil
    ) throws {
        schemaVersion = Self.currentSchemaVersion
        self.revision = revision
        self.activationIdentifier = activationIdentifier
        self.profileRulesProxy = profileRulesProxy
        self.routeProxyEndpoints = routeProxyEndpoints
        self.encodedCaptureSnapshot = encodedCaptureSnapshot
        try validate()
    }

    public func validate() throws {
        guard schemaVersion == 1 || schemaVersion == Self.currentSchemaVersion else {
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
        case let .encodedPayloadTooLarge(actual, maximum):
            "DNS proxy bootstrap is \(actual) bytes; the maximum is \(maximum)."
        }
    }
}
