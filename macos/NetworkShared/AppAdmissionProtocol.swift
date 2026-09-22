import Foundation

public struct AppAdmissionBootstrap: Codable, Equatable, Sendable {
    public static let version = 1
    public let version: Int
    public let activation: String
    public let port: UInt16
    public let key: String

    public init(activation: String, port: UInt16, key: String) throws {
        guard port > 0, Data(base64Encoded: key)?.count == 32 else {
            throw AppAdmissionProtocolError.invalidBootstrap
        }
        self.version = Self.version
        self.activation = activation
        self.port = port
        self.key = key
    }
}

public struct AppAdmissionSource: Codable, Equatable, Sendable {
    public let processId: Int32?
    public let userId: UInt32?
    public let processStart: String?
    public let executablePath: String?
    public let bundleId: String?
    public let signingId: String?
    public let teamId: String?
    public init(processId: Int32?, userId: UInt32?, processStart: String?, executablePath: String?, bundleId: String?, signingId: String?, teamId: String?) {
        self.processId = processId; self.userId = userId; self.processStart = processStart
        self.executablePath = executablePath; self.bundleId = bundleId; self.signingId = signingId; self.teamId = teamId
    }
}

public struct AppAdmissionRequest: Codable, Equatable, Sendable {
    public let version: Int
    public let activation: String
    public let nonce: String
    public let flowId: String
    public let kind: String
    public let network: String
    public let host: String
    public let hostname: String?
    public let port: UInt16
    public let source: AppAdmissionSource
    public init(version: Int, activation: String, nonce: String, flowId: String, kind: String, network: String, host: String, hostname: String?, port: UInt16, source: AppAdmissionSource) {
        self.version = version; self.activation = activation; self.nonce = nonce; self.flowId = flowId
        self.kind = kind; self.network = network; self.host = host; self.hostname = hostname; self.port = port; self.source = source
    }
}

public struct AppAdmissionReply: Codable, Equatable, Sendable {
    public let version: Int
    public let activation: String
    public let nonce: String
    public let action: String
    public let generation: UInt64
    public let host: String
    public let port: UInt16
    public let rule: String
    public let chain: [String]
    public let relayPort: UInt16?
    public let lease: String?
    public let password: String?
}

public enum AppAdmissionProtocolError: Error, Equatable, Sendable {
    case invalidBootstrap
    case invalidRequest
    case invalidReply
    case authenticationFailed
    case timeout
    case unavailable
    case frameTooLarge
}
