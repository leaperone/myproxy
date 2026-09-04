import Foundation

/// Platform-neutral representation of an endpoint supplied by NetworkExtension.
/// The provider intentionally passes the port as text because the legacy
/// `NWHostEndpoint` API exposes it that way.
public struct FlowRemoteEndpoint: Codable, Hashable, Sendable {
    public let host: String
    public let port: String

    public init(host: String, port: String) {
        self.host = host
        self.port = port
    }

    public init(host: String, port: UInt16) {
        self.init(host: host, port: String(port))
    }
}

/// Values copied from `NEFlowMetaData` before entering the shared decision layer.
public struct FlowApplicationMetadata: Codable, Hashable, Sendable {
    public let sourceAppAuditToken: Data?
    public let sourceAppUniqueIdentifier: Data
    public let sourceAppSigningIdentifier: String

    public init(
        sourceAppAuditToken: Data?,
        sourceAppUniqueIdentifier: Data,
        sourceAppSigningIdentifier: String
    ) {
        self.sourceAppAuditToken = sourceAppAuditToken
        self.sourceAppUniqueIdentifier = sourceAppUniqueIdentifier
        self.sourceAppSigningIdentifier = sourceAppSigningIdentifier
    }
}

public enum FlowContextConversionFailure: Error, Codable, Hashable, Sendable {
    case missingSourceAppAuditToken
    case identityUnavailable(ProcessIdentityResolutionFailure)
    case identityAuditTokenMismatch
    case signingIdentifierMismatch(metadata: String, resolved: String?)
    case emptyRemoteHost
    case invalidRemotePort(String)
    case unsupportedRemoteEndpoint
}

extension FlowContextConversionFailure: CustomStringConvertible {
    public var description: String {
        switch self {
        case .missingSourceAppAuditToken:
            return "The flow does not contain a source application audit token"
        case let .identityUnavailable(failure):
            return "The source process identity is unavailable: \(failure)"
        case .identityAuditTokenMismatch:
            return "The resolved process identity does not belong to the flow audit token"
        case let .signingIdentifierMismatch(metadata, resolved):
            let resolvedDescription = resolved ?? "unsigned code"
            return "The flow signing identifier \(metadata) does not match \(resolvedDescription)"
        case .emptyRemoteHost:
            return "The flow remote endpoint has an empty host"
        case let .invalidRemotePort(port):
            return "The flow remote endpoint has an invalid port: \(port)"
        case .unsupportedRemoteEndpoint:
            return "The flow uses an endpoint type that cannot be matched by capture rules"
        }
    }
}

/// A fully resolved source can be used by every source matcher. When process
/// inspection is unavailable, the kernel-supplied NetworkExtension metadata is
/// retained so destination-only rules and application-identifier patterns can
/// still be evaluated. Strict source matchers remain unavailable in that case.
public enum FlowContextResolution: Codable, Hashable, Sendable {
    case resolved(context: FlowContext, processIdentity: ResolvedProcessIdentity)
    case kernelMetadataOnly(context: FlowContext, sourceFailure: FlowContextConversionFailure)
    case failOpen(FlowContextConversionFailure)

    public var context: FlowContext? {
        switch self {
        case let .resolved(context, _), let .kernelMetadataOnly(context, _):
            context
        case .failOpen:
            nil
        }
    }

    public var processIdentity: ResolvedProcessIdentity? {
        guard case let .resolved(_, identity) = self else { return nil }
        return identity
    }

    fileprivate var sourceEvaluationMode: CaptureRuleEngine.SourceEvaluationMode {
        switch self {
        case .resolved:
            .verified
        case .kernelMetadataOnly:
            .kernelMetadataOnly
        case .failOpen:
            .unavailable
        }
    }
}

public struct FlowContextBuilder: Sendable {
    public init() {}

    public func resolve(
        endpoint: FlowRemoteEndpoint,
        remoteHostname: String? = nil,
        metadata: FlowApplicationMetadata,
        identityResolution: ProcessIdentityResolution,
        transportProtocol: TransportProtocol,
        isTrustedMyproxyComponent: Bool = false
    ) -> FlowContextResolution {
        let destination: FlowDestination
        do {
            destination = try makeDestination(
                endpoint: endpoint,
                remoteHostname: remoteHostname
            )
        } catch let failure as FlowContextConversionFailure {
            return .failOpen(failure)
        } catch {
            return .failOpen(.unsupportedRemoteEndpoint)
        }

        guard let auditTokenData = metadata.sourceAppAuditToken else {
            return kernelMetadataResolution(
                metadata: metadata,
                destination: destination,
                transportProtocol: transportProtocol,
                failure: .missingSourceAppAuditToken,
                isTrustedMyproxyComponent: isTrustedMyproxyComponent
            )
        }

        let identity: ResolvedProcessIdentity
        switch identityResolution {
        case let .resolved(value):
            identity = value
        case let .unavailable(failure):
            return kernelMetadataResolution(
                metadata: metadata,
                destination: destination,
                transportProtocol: transportProtocol,
                failure: .identityUnavailable(failure),
                isTrustedMyproxyComponent: isTrustedMyproxyComponent
            )
        }

        guard identity.auditToken.data == auditTokenData else {
            return kernelMetadataResolution(
                metadata: metadata,
                destination: destination,
                transportProtocol: transportProtocol,
                failure: .identityAuditTokenMismatch,
                isTrustedMyproxyComponent: isTrustedMyproxyComponent
            )
        }

        let metadataSigningIdentifier = metadata.sourceAppSigningIdentifier
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let resolvedSigningIdentifier: String?
        switch identity.codeSigning {
        case .unsigned:
            resolvedSigningIdentifier = nil
        case let .signed(signing):
            resolvedSigningIdentifier = signing.signingIdentifier
        }
        if !metadataSigningIdentifier.isEmpty,
           metadataSigningIdentifier != resolvedSigningIdentifier {
            return kernelMetadataResolution(
                metadata: metadata,
                destination: destination,
                transportProtocol: transportProtocol,
                failure: .signingIdentifierMismatch(
                    metadata: metadataSigningIdentifier,
                    resolved: resolvedSigningIdentifier
                ),
                isTrustedMyproxyComponent: isTrustedMyproxyComponent
            )
        }

        let signingIdentity: SignedCodeIdentity?
        if case let .signed(value) = identity.codeSigning {
            signingIdentity = value
        } else {
            signingIdentity = nil
        }
        let uniqueIdentifier = metadata.sourceAppUniqueIdentifier.isEmpty
            ? nil
            : metadata.sourceAppUniqueIdentifier
        let source = FlowSource(
            processIdentifier: identity.processIdentifier,
            auditToken: identity.auditToken.data,
            processStartTime: identity.processStartTime,
            sourceAppUniqueIdentifier: uniqueIdentifier,
            userID: identity.effectiveUserID,
            executablePath: identity.executablePath,
            executableSHA256: nil,
            designatedRequirement: signingIdentity?.designatedRequirement,
            signingIdentifier: signingIdentity?.signingIdentifier,
            teamIdentifier: signingIdentity?.teamIdentifier,
            bundleIdentifier: signingIdentity?.securedBundleIdentifier,
            isTrustedMyproxyComponent: isTrustedMyproxyComponent
        )
        return .resolved(
            context: FlowContext(
                source: source,
                destination: destination,
                transportProtocol: transportProtocol
            ),
            processIdentity: identity
        )
    }

    private func kernelMetadataResolution(
        metadata: FlowApplicationMetadata,
        destination: FlowDestination,
        transportProtocol: TransportProtocol,
        failure: FlowContextConversionFailure,
        isTrustedMyproxyComponent: Bool
    ) -> FlowContextResolution {
        let signingIdentifier = metadata.sourceAppSigningIdentifier
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let source = FlowSource(
            // PID, UID, paths, and requirements must not be inferred when
            // process inspection failed. Only fields supplied by NEFlowMetaData
            // are retained; an exact product-owned signing identifier may mark
            // myproxy itself trusted solely to prevent a capture loop.
            processIdentifier: 0,
            auditToken: metadata.sourceAppAuditToken ?? Data(),
            sourceAppUniqueIdentifier: metadata.sourceAppUniqueIdentifier.isEmpty
                ? nil
                : metadata.sourceAppUniqueIdentifier,
            userID: 0,
            signingIdentifier: signingIdentifier.isEmpty ? nil : signingIdentifier,
            isTrustedMyproxyComponent: isTrustedMyproxyComponent
        )
        return .kernelMetadataOnly(
            context: FlowContext(
                source: source,
                destination: destination,
                transportProtocol: transportProtocol
            ),
            sourceFailure: failure
        )
    }

    private func makeDestination(
        endpoint: FlowRemoteEndpoint,
        remoteHostname: String?
    ) throws -> FlowDestination {
        var host = endpoint.host.trimmingCharacters(in: .whitespacesAndNewlines)
        if host.hasPrefix("["), host.hasSuffix("]") {
            host.removeFirst()
            host.removeLast()
        }
        guard !host.isEmpty else {
            throw FlowContextConversionFailure.emptyRemoteHost
        }
        guard let port = UInt16(endpoint.port), port > 0 else {
            throw FlowContextConversionFailure.invalidRemotePort(endpoint.port)
        }

        let suppliedHostname = remoteHostname?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let normalizedHostname = suppliedHostname?.isEmpty == false
            ? suppliedHostname
            : nil
        if let address = try? IPAddress(host) {
            return try FlowDestination(
                hostname: normalizedHostname,
                ipAddress: address,
                port: port
            )
        }
        return try FlowDestination(
            hostname: normalizedHostname ?? host,
            port: port
        )
    }
}

public enum CaptureConfigurationLoadFailure: Error, Codable, Hashable, Sendable {
    case missingEncodedSnapshot
    case encodedSnapshotTooLarge(actual: Int, maximum: Int)
    case invalidEncodedSnapshot(String)
    case tooManyRules(actual: Int, maximum: Int)
}

public enum CaptureConfigurationLoadResult: Codable, Hashable, Sendable {
    case loaded(CaptureConfigurationSnapshot)
    case failOpen(CaptureConfigurationLoadFailure)

    public var snapshot: CaptureConfigurationSnapshot? {
        guard case let .loaded(snapshot) = self else { return nil }
        return snapshot
    }
}

/// A validated capture snapshot together with its compiled rule indexes.
///
/// Network providers keep one instance for the lifetime of a configuration
/// revision so large hostname groups are compiled once, not once per flow.
public struct PreparedCaptureConfiguration: Sendable {
    public let loadResult: CaptureConfigurationLoadResult
    fileprivate let ruleEngine: CaptureRuleEngine?

    public init(_ loadResult: CaptureConfigurationLoadResult) {
        self.loadResult = loadResult
        ruleEngine = loadResult.snapshot.map(CaptureRuleEngine.init(snapshot:))
    }

    var containsCompiledRuleEngine: Bool { ruleEngine != nil }
}

public struct CaptureConfigurationSnapshotLoader: Sendable {
    public static let maximumEncodedSize = 8 * 1_024 * 1_024
    public static let maximumRuleCount = 10_000

    public init() {}

    /// Loads a snapshot encoded directly with `JSONEncoder`. Both Foundation's
    /// default date representation and ISO-8601 are accepted so the provider can
    /// consume data produced by the runtime store without weakening validation.
    public func load(_ data: Data?) -> CaptureConfigurationLoadResult {
        guard let data else {
            return .failOpen(.missingEncodedSnapshot)
        }
        guard data.count <= Self.maximumEncodedSize else {
            return .failOpen(.encodedSnapshotTooLarge(
                actual: data.count,
                maximum: Self.maximumEncodedSize
            ))
        }

        do {
            let snapshot: CaptureConfigurationSnapshot
            do {
                snapshot = try JSONDecoder().decode(
                    CaptureConfigurationSnapshot.self,
                    from: data
                )
            } catch {
                let decoder = JSONDecoder()
                decoder.dateDecodingStrategy = .iso8601
                snapshot = try decoder.decode(CaptureConfigurationSnapshot.self, from: data)
            }
            try snapshot.validate()
            guard snapshot.rules.count <= Self.maximumRuleCount else {
                return .failOpen(.tooManyRules(
                    actual: snapshot.rules.count,
                    maximum: Self.maximumRuleCount
                ))
            }
            return .loaded(snapshot)
        } catch {
            return .failOpen(.invalidEncodedSnapshot(String(describing: error)))
        }
    }
}

public enum FlowTrafficDisposition: Codable, Hashable, Sendable {
    case direct
    case reject
    case mihomo(MihomoRoute)
    case failOpen
}

public enum FlowTrafficDecisionReason: Codable, Hashable, Sendable {
    case captureDisabled
    case configurationUnavailable(CaptureConfigurationLoadFailure)
    case contextUnavailable(FlowContextConversionFailure)
    case rule(RuleDecisionCause)
    case mihomoUnavailable(rule: RuleDecisionCause, fallback: UnavailableFallback)
}

public struct FlowTrafficDecision: Codable, Hashable, Sendable {
    public let disposition: FlowTrafficDisposition
    public let reason: FlowTrafficDecisionReason
    /// Optional for wire compatibility with activity records written before
    /// structured rule evidence was introduced.
    public let ruleEvidence: CaptureRuleDecisionEvidence?

    public init(
        disposition: FlowTrafficDisposition,
        reason: FlowTrafficDecisionReason,
        ruleEvidence: CaptureRuleDecisionEvidence? = nil
    ) {
        self.disposition = disposition
        self.reason = reason
        self.ruleEvidence = ruleEvidence
    }
}

public struct FlowTrafficDecisionAdapter: Sendable {
    public init() {}

    public func decide(
        configuration: CaptureConfigurationLoadResult,
        context: FlowContextResolution,
        captureEnabled: Bool,
        mihomoAvailable: Bool
    ) -> FlowTrafficDecision {
        decide(
            preparedConfiguration: PreparedCaptureConfiguration(configuration),
            context: context,
            captureEnabled: captureEnabled,
            mihomoAvailable: mihomoAvailable
        )
    }

    public func decide(
        preparedConfiguration: PreparedCaptureConfiguration,
        context: FlowContextResolution,
        captureEnabled: Bool,
        mihomoAvailable: Bool
    ) -> FlowTrafficDecision {
        let configuration = preparedConfiguration.loadResult
        guard captureEnabled else {
            return FlowTrafficDecision(
                disposition: .failOpen,
                reason: .captureDisabled,
                ruleEvidence: CaptureRuleDecisionEvidence(outcome: .captureDisabled)
            )
        }
        guard case let .loaded(snapshot) = configuration else {
            guard case let .failOpen(failure) = configuration else {
                preconditionFailure("CaptureConfigurationLoadResult gained an unhandled case")
            }
            return FlowTrafficDecision(
                disposition: .failOpen,
                reason: .configurationUnavailable(failure),
                ruleEvidence: CaptureRuleDecisionEvidence(outcome: .configurationUnavailable)
            )
        }
        guard let flowContext = context.context else {
            guard case let .failOpen(failure) = context else {
                preconditionFailure("A context-bearing resolution unexpectedly contained no context")
            }
            return FlowTrafficDecision(
                disposition: .failOpen,
                reason: .contextUnavailable(failure),
                ruleEvidence: CaptureRuleDecisionEvidence(
                    outcome: .contextUnavailable,
                    contextUnavailableReason: Self.contextUnavailableReason(failure)
                )
            )
        }

        // A loaded prepared configuration always owns the engine compiled from
        // this snapshot. Keep a defensive fallback for future enum evolution.
        let ruleEngine = preparedConfiguration.ruleEngine
            ?? CaptureRuleEngine(snapshot: snapshot)
        let ruleDecision = ruleEngine.evaluate(
            flowContext,
            sourceEvaluationMode: context.sourceEvaluationMode
        )
        switch ruleDecision.action {
        case .direct:
            return FlowTrafficDecision(
                disposition: .direct,
                reason: .rule(ruleDecision.cause),
                ruleEvidence: ruleDecision.evidence
            )
        case .reject:
            return FlowTrafficDecision(
                disposition: .reject,
                reason: .rule(ruleDecision.cause),
                ruleEvidence: ruleDecision.evidence
            )
        case let .mihomo(route):
            if mihomoAvailable {
                return FlowTrafficDecision(
                    disposition: .mihomo(route),
                    reason: .rule(ruleDecision.cause),
                    ruleEvidence: ruleDecision.evidence
                )
            }
            let disposition: FlowTrafficDisposition = switch ruleDecision.unavailableFallback {
            case .direct: .direct
            case .reject: .reject
            }
            return FlowTrafficDecision(
                disposition: disposition,
                reason: .mihomoUnavailable(
                    rule: ruleDecision.cause,
                    fallback: ruleDecision.unavailableFallback
                ),
                ruleEvidence: ruleDecision.evidence
            )
        }
    }

    private static func contextUnavailableReason(
        _ failure: FlowContextConversionFailure
    ) -> RuleContextUnavailableReason {
        switch failure {
        case .missingSourceAppAuditToken:
            .missingSourceApplicationAuditToken
        case .identityUnavailable:
            .sourceIdentityResolutionFailed
        case .identityAuditTokenMismatch:
            .sourceIdentityAuditTokenMismatch
        case .signingIdentifierMismatch:
            .sourceSigningIdentifierMismatch
        case .emptyRemoteHost:
            .emptyRemoteHost
        case .invalidRemotePort:
            .invalidRemotePort
        case .unsupportedRemoteEndpoint:
            .unsupportedRemoteEndpoint
        }
    }
}
