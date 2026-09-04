import Foundation

/// `audit_token_t` is eight 32-bit words on supported macOS releases.
/// Keeping this wrapper in the shared target makes malformed provider metadata
/// impossible to represent after decoding.
public struct SourceAppAuditToken: Codable, Hashable, Sendable {
    public static let byteCount = 32

    public let data: Data

    public init(_ data: Data) throws {
        guard data.count == Self.byteCount else {
            throw ProcessIdentityResolutionFailure.invalidAuditTokenLength(
                expected: Self.byteCount,
                actual: data.count
            )
        }
        self.data = data
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        let data = try container.decode(Data.self)
        do {
            try self.init(data)
        } catch {
            throw DecodingError.dataCorruptedError(
                in: container,
                debugDescription: String(describing: error)
            )
        }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(data)
    }
}

public struct SignedCodeIdentity: Codable, Hashable, Sendable {
    /// The signing identifier sealed into the code signature. This is useful for
    /// display and indexing, but must not be used as the sole trust decision.
    public let signingIdentifier: String
    public let teamIdentifier: String?
    public let designatedRequirement: String
    public let codeDirectoryHash: Data?
    public let securedBundleIdentifier: String?
    public let mainExecutablePath: String?
    public let isApplePlatformCode: Bool

    public init(
        signingIdentifier: String,
        teamIdentifier: String?,
        designatedRequirement: String,
        codeDirectoryHash: Data?,
        securedBundleIdentifier: String?,
        mainExecutablePath: String?,
        isApplePlatformCode: Bool
    ) {
        self.signingIdentifier = signingIdentifier
        self.teamIdentifier = teamIdentifier
        self.designatedRequirement = designatedRequirement
        self.codeDirectoryHash = codeDirectoryHash
        self.securedBundleIdentifier = securedBundleIdentifier
        self.mainExecutablePath = mainExecutablePath
        self.isApplePlatformCode = isApplePlatformCode
    }
}

public enum ProcessCodeSigningIdentity: Codable, Hashable, Sendable {
    case unsigned
    case signed(SignedCodeIdentity)
}

public struct ResolvedProcessIdentity: Codable, Hashable, Sendable {
    public let auditToken: SourceAppAuditToken
    public let processIdentifier: Int32
    public let processVersion: Int32
    public let processStartTime: ProcessStartTime?
    public let effectiveUserID: UInt32
    public let auditUserID: UInt32
    public let executablePath: String
    public let codeSigning: ProcessCodeSigningIdentity

    public init(
        auditToken: SourceAppAuditToken,
        processIdentifier: Int32,
        processVersion: Int32,
        processStartTime: ProcessStartTime? = nil,
        effectiveUserID: UInt32,
        auditUserID: UInt32,
        executablePath: String,
        codeSigning: ProcessCodeSigningIdentity
    ) {
        self.auditToken = auditToken
        self.processIdentifier = processIdentifier
        self.processVersion = processVersion
        self.processStartTime = processStartTime
        self.effectiveUserID = effectiveUserID
        self.auditUserID = auditUserID
        self.executablePath = executablePath
        self.codeSigning = codeSigning
    }
}

public enum ProcessIdentityResolutionFailure: Error, Codable, Hashable, Sendable {
    case unsupportedAuditTokenLayout(expected: Int, actual: Int)
    case invalidAuditTokenLength(expected: Int, actual: Int)
    case invalidProcessIdentifier(Int32)
    case processNoLongerExists
    case executablePathPermissionDenied(errno: Int32)
    case executablePathUnavailable(errno: Int32)
    case emptyExecutablePath
    case codeObjectLookupFailed(status: Int32)
    case staticCodeLookupFailed(status: Int32)
    case codeSignatureInvalid(status: Int32)
    case signingInformationFailed(status: Int32)
    case malformedSigningInformation
    case designatedRequirementFailed(status: Int32)
    case requirementStringFailed(status: Int32)
}

extension ProcessIdentityResolutionFailure: CustomStringConvertible {
    public var description: String {
        switch self {
        case let .unsupportedAuditTokenLayout(expected, actual):
            return "Unsupported audit_token_t layout (shared model: \(expected) bytes, SDK: \(actual) bytes)"
        case let .invalidAuditTokenLength(expected, actual):
            return "Invalid source application audit token length (expected \(expected), received \(actual))"
        case let .invalidProcessIdentifier(pid):
            return "Audit token contains an invalid process identifier: \(pid)"
        case .processNoLongerExists:
            return "The source process no longer exists"
        case let .executablePathPermissionDenied(code):
            return "Reading the source executable path was denied (errno \(code))"
        case let .executablePathUnavailable(code):
            return "The source executable path is unavailable (errno \(code))"
        case .emptyExecutablePath:
            return "The source executable path was empty"
        case let .codeObjectLookupFailed(status):
            return "Looking up the running code by audit token failed (OSStatus \(status))"
        case let .staticCodeLookupFailed(status):
            return "Resolving the running code's static code failed (OSStatus \(status))"
        case let .codeSignatureInvalid(status):
            return "The running code signature is invalid (OSStatus \(status))"
        case let .signingInformationFailed(status):
            return "Reading code signing information failed (OSStatus \(status))"
        case .malformedSigningInformation:
            return "Code signing information was incomplete or malformed"
        case let .designatedRequirementFailed(status):
            return "Reading the designated requirement failed (OSStatus \(status))"
        case let .requirementStringFailed(status):
            return "Serializing the designated requirement failed (OSStatus \(status))"
        }
    }

    /// A caller must bypass interception when identity cannot be established.
    public var requiresFailOpen: Bool { true }
}

public enum ProcessIdentityResolution: Codable, Hashable, Sendable {
    case resolved(ResolvedProcessIdentity)
    case unavailable(ProcessIdentityResolutionFailure)

    public var identity: ResolvedProcessIdentity? {
        guard case let .resolved(identity) = self else { return nil }
        return identity
    }

    /// Legacy signal retained for source compatibility. Flow disposition must
    /// now be decided by `FlowContextBuilder`, which can evaluate bounded
    /// kernel metadata even when this value is `true`.
    public var shouldFailOpen: Bool {
        if case .unavailable = self { return true }
        return false
    }
}

/// Identifies myproxy data-plane processes whose traffic must never be sent back
/// through the transparent proxy. The host application is intentionally not a
/// member: its update, subscription, and API traffic may use App Routing like
/// any other application. A resolved identity requires both the exact signing
/// identifier and Developer ID team. The kernel-published
/// `NEFlowMetaData.sourceAppSigningIdentifier` can also be used when process
/// inspection is unavailable because it is not supplied by the application.
public struct TrustedMyproxyComponentPolicy: Sendable {
    public static let teamIdentifier = "5UAHRS482C"

    private static let signingIdentifiers: Set<String> = [
        "local.harry.myproxy.mihomo",
        "local.harry.myproxy.network-extension",
    ]

    public init() {}

    public func contains(_ resolution: ProcessIdentityResolution) -> Bool {
        guard case let .resolved(identity) = resolution,
              case let .signed(signing) = identity.codeSigning
        else {
            return false
        }
        return signing.teamIdentifier == Self.teamIdentifier
            && Self.signingIdentifiers.contains(signing.signingIdentifier)
    }

    public func contains(metadataSigningIdentifier value: String) -> Bool {
        Self.signingIdentifiers.contains(
            value.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        )
    }
}
