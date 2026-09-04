import Foundation
import MyproxyNetworkShared

/// Versioned messages exchanged between the host app and the transparent
/// provider through `NETunnelProviderSession.sendProviderMessage`.
///
/// The DNS provider cannot receive provider messages directly. The host stages
/// its bootstrap and reads its runtime report through the authenticated
/// transparent-provider channel, with `providerConfiguration` retained as a
/// cold-process fallback.
enum ProviderControlCommand: String, Codable, Sendable {
    case status
    case prepareDNS
    case dnsStatus
    case applyConfiguration
    case quiesce
    case activity
    case clearActivity
}

struct ProviderControlRequest: Codable, Sendable {
    static let currentProtocolVersion = 3

    let protocolVersion: Int
    let command: ProviderControlCommand
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
    let activityCursor: UInt64?
    let activityLimit: Int?
}

struct ProviderControlResponse: Codable, Sendable {
    let protocolVersion: Int
    let accepted: Bool
    let provider: String
    let revision: UInt64
    let running: Bool
    let captureEnabled: Bool
    let failOpen: Bool
    let message: String?
    let activityBatch: AppRoutingActivityBatch?
    let dnsRuntimeReport: DNSProxyRuntimeReport?
}

enum ProviderConfigurationKey {
    static let revision = "revision"
    static let captureEnabled = "captureEnabled"
    static let failOpen = "failOpen"
    static let activationIdentifier = "activationIdentifier"
    static let dnsProxyBootstrap = "dnsProxyBootstrap"
    /// JSON-encoded `CaptureConfigurationSnapshot` stored as `Data`/`NSData`.
    static let captureConfigurationSnapshot = "captureConfigurationSnapshot"
    /// JSON-encoded `[MihomoRouteProxyEndpoint]` stored as `Data`/`NSData`.
    static let mihomoRouteProxyCatalog = "mihomoRouteProxyCatalog"
    static let mihomoSOCKSHost = "mihomoSOCKSHost"
    static let mihomoSOCKSPort = "mihomoSOCKSPort"
    static let mihomoSOCKSUsername = "mihomoSOCKSUsername"
    static let mihomoSOCKSPassword = "mihomoSOCKSPassword"
}

enum ProviderControlCodec {
    static func decode(_ data: Data) throws -> ProviderControlRequest {
        try JSONDecoder().decode(ProviderControlRequest.self, from: data)
    }

    static func encode(_ response: ProviderControlResponse) -> Data? {
        try? JSONEncoder().encode(response)
    }
}
