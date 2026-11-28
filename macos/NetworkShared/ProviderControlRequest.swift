import Foundation

/// Versioned messages exchanged between the host app and the transparent
/// provider through `NETunnelProviderSession.sendProviderMessage`.
///
/// The DNS provider cannot receive provider messages directly. The host stages
/// its bootstrap and reads its runtime report through the authenticated
/// transparent-provider channel, with `providerConfiguration` retained as a
/// cold-process fallback.
public enum ProviderControlCommand: String, Codable, Sendable {
    case status
    case prepareDNS
    case dnsStatus
    case applyConfiguration
    case quiesce
    case activity
    case clearActivity
}

public struct ProviderControlRequest: Codable, Sendable {
    public static let currentProtocolVersion = 3

    public let protocolVersion: Int
    public let command: ProviderControlCommand
    public let revision: UInt64?
    public let activationIdentifier: UUID?
    public let dnsProxyBootstrap: Data?
    public let captureEnabled: Bool?
    public let failOpen: Bool?
    public let captureConfigurationSnapshot: Data?
    public let mihomoRouteProxyCatalog: Data?
    public let mihomoSOCKSHost: String?
    public let mihomoSOCKSPort: UInt16?
    public let mihomoSOCKSUsername: String?
    public let mihomoSOCKSPassword: String?
    public let activityCursor: UInt64?
    public let activityLimit: Int?

    public init(
        command: ProviderControlCommand,
        revision: UInt64? = nil,
        activationIdentifier: UUID? = nil,
        dnsProxyBootstrap: Data? = nil,
        captureEnabled: Bool? = nil,
        failOpen: Bool? = nil,
        captureConfigurationSnapshot: Data? = nil,
        mihomoRouteProxyCatalog: Data? = nil,
        mihomoSOCKSHost: String? = nil,
        mihomoSOCKSPort: UInt16? = nil,
        mihomoSOCKSUsername: String? = nil,
        mihomoSOCKSPassword: String? = nil,
        activityCursor: UInt64? = nil,
        activityLimit: Int? = nil
    ) {
        protocolVersion = Self.currentProtocolVersion
        self.command = command
        self.revision = revision
        self.activationIdentifier = activationIdentifier
        self.dnsProxyBootstrap = dnsProxyBootstrap
        self.captureEnabled = captureEnabled
        self.failOpen = failOpen
        self.captureConfigurationSnapshot = captureConfigurationSnapshot
        self.mihomoRouteProxyCatalog = mihomoRouteProxyCatalog
        self.mihomoSOCKSHost = mihomoSOCKSHost
        self.mihomoSOCKSPort = mihomoSOCKSPort
        self.mihomoSOCKSUsername = mihomoSOCKSUsername
        self.mihomoSOCKSPassword = mihomoSOCKSPassword
        self.activityCursor = activityCursor
        self.activityLimit = activityLimit
    }
}

public enum ProviderConfigurationKey {
    public static let revision = "revision"
    public static let captureEnabled = "captureEnabled"
    public static let failOpen = "failOpen"
    public static let activationIdentifier = "activationIdentifier"
    public static let dnsProxyBootstrap = "dnsProxyBootstrap"
    /// JSON-encoded `CaptureConfigurationSnapshot` stored as `Data`/`NSData`.
    public static let captureConfigurationSnapshot = "captureConfigurationSnapshot"
    /// JSON-encoded `[MihomoRouteProxyEndpoint]` stored as `Data`/`NSData`.
    public static let mihomoRouteProxyCatalog = "mihomoRouteProxyCatalog"
    public static let mihomoSOCKSHost = "mihomoSOCKSHost"
    public static let mihomoSOCKSPort = "mihomoSOCKSPort"
    public static let mihomoSOCKSUsername = "mihomoSOCKSUsername"
    public static let mihomoSOCKSPassword = "mihomoSOCKSPassword"
}
