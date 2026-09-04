import Foundation

public enum BuiltInBypassReason: String, Codable, Hashable, Sendable {
    case trustedmyproxyComponent
    case loopback
    case linkLocal
    case multicast
    case unspecifiedAddress
}

public enum RuleDecisionCause: Codable, Hashable, Sendable {
    case builtInBypass(BuiltInBypassReason)
    case matchedRule(String)
    case defaultDirect
}

/// The strength of the identity check that selected a source matcher.
///
/// This deliberately describes the check without exporting the audit token,
/// designated-requirement text, or executable hash used to perform it.
public enum RuleSourceIdentityAssurance: String, Codable, Hashable, Sendable {
    case verifiedCodeSignatureRequirement
    case identifierPattern
    case exactExecutablePath
    case exactExecutablePathAndCodeSignatureRequirement
    case exactExecutablePathAndSHA256
    case exactExecutablePathCodeSignatureRequirementAndSHA256
    case exactAuditToken
    case exactProcessStartTimeAndExecutablePath
    case exactUserIdentifier
}

public enum RuleApplicationIdentifierField: String, Codable, Hashable, Sendable {
    case bundleIdentifier
    case signingIdentifier
    case executableName
}

/// Privacy-bounded, display-safe details for an application matcher.
public struct RuleApplicationSourceEvidence: Codable, Hashable, Sendable {
    public static let maximumIdentifierLength = 255

    public let signingIdentifier: String?
    public let teamIdentifier: String?
    public let bundleIdentifier: String?

    public init(
        signingIdentifier: String?,
        teamIdentifier: String?,
        bundleIdentifier: String?
    ) {
        self.signingIdentifier = Self.bounded(signingIdentifier)
        self.teamIdentifier = Self.bounded(teamIdentifier)
        self.bundleIdentifier = Self.bounded(bundleIdentifier)
    }

    private enum CodingKeys: String, CodingKey {
        case signingIdentifier
        case teamIdentifier
        case bundleIdentifier
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        self.init(
            signingIdentifier: try container.decodeIfPresent(String.self, forKey: .signingIdentifier),
            teamIdentifier: try container.decodeIfPresent(String.self, forKey: .teamIdentifier),
            bundleIdentifier: try container.decodeIfPresent(String.self, forKey: .bundleIdentifier)
        )
    }

    private static func bounded(_ value: String?) -> String? {
        value.map { String($0.prefix(maximumIdentifierLength)) }
    }
}

public struct RuleApplicationPatternEvidence: Codable, Hashable, Sendable {
    public static let maximumPatternLength = 255

    public let pattern: String
    public let matchedField: RuleApplicationIdentifierField

    public init(pattern: String, matchedField: RuleApplicationIdentifierField) {
        self.pattern = String(pattern.prefix(Self.maximumPatternLength))
        self.matchedField = matchedField
    }

    private enum CodingKeys: String, CodingKey {
        case pattern
        case matchedField
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        self.init(
            pattern: try container.decode(String.self, forKey: .pattern),
            matchedField: try container.decode(RuleApplicationIdentifierField.self, forKey: .matchedField)
        )
    }
}

public struct RuleExecutableSourceEvidence: Codable, Hashable, Sendable {
    public static let maximumPathLength = 1_024

    public let canonicalPath: String
    public let assurance: RuleSourceIdentityAssurance

    public init(canonicalPath: String, assurance: RuleSourceIdentityAssurance) {
        self.canonicalPath = String(canonicalPath.prefix(Self.maximumPathLength))
        self.assurance = assurance
    }

    private enum CodingKeys: String, CodingKey {
        case canonicalPath
        case assurance
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        self.init(
            canonicalPath: try container.decode(String.self, forKey: .canonicalPath),
            assurance: try container.decode(RuleSourceIdentityAssurance.self, forKey: .assurance)
        )
    }
}

public struct RuleProcessInstanceSourceEvidence: Codable, Hashable, Sendable {
    public static let maximumPathLength = 1_024

    public let processIdentifier: Int32
    public let canonicalExecutablePath: String?
    public let assurance: RuleSourceIdentityAssurance

    public init(
        processIdentifier: Int32,
        canonicalExecutablePath: String?,
        assurance: RuleSourceIdentityAssurance
    ) {
        self.processIdentifier = processIdentifier
        self.canonicalExecutablePath = canonicalExecutablePath.map {
            String($0.prefix(Self.maximumPathLength))
        }
        self.assurance = assurance
    }

    private enum CodingKeys: String, CodingKey {
        case processIdentifier
        case canonicalExecutablePath
        case assurance
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        self.init(
            processIdentifier: try container.decode(Int32.self, forKey: .processIdentifier),
            canonicalExecutablePath: try container.decodeIfPresent(
                String.self,
                forKey: .canonicalExecutablePath
            ),
            assurance: try container.decode(RuleSourceIdentityAssurance.self, forKey: .assurance)
        )
    }
}

public enum RuleSourceMatchEvidence: Codable, Hashable, Sendable {
    case unconstrained
    case application(RuleApplicationSourceEvidence)
    case applicationIdentifierPattern(RuleApplicationPatternEvidence)
    case executable(RuleExecutableSourceEvidence)
    case processInstance(RuleProcessInstanceSourceEvidence)
    case userID(UInt32)

    public var identityAssurance: RuleSourceIdentityAssurance? {
        switch self {
        case .unconstrained:
            nil
        case .application:
            .verifiedCodeSignatureRequirement
        case .applicationIdentifierPattern:
            .identifierPattern
        case let .executable(executable):
            executable.assurance
        case let .processInstance(process):
            process.assurance
        case .userID:
            .exactUserIdentifier
        }
    }
}

public struct RuleHostDestinationEvidence: Codable, Hashable, Sendable {
    public static let maximumHostLength = 253

    public let kind: HostMatcher.Kind
    public let value: String

    public init(kind: HostMatcher.Kind, value: String) {
        self.kind = kind
        self.value = String(value.prefix(Self.maximumHostLength))
    }

    private enum CodingKeys: String, CodingKey {
        case kind
        case value
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        self.init(
            kind: try container.decode(HostMatcher.Kind.self, forKey: .kind),
            value: try container.decode(String.self, forKey: .value)
        )
    }
}

public struct RuleHostPatternDestinationEvidence: Codable, Hashable, Sendable {
    public static let maximumPatternLength = 253

    public let pattern: String

    public init(pattern: String) {
        self.pattern = String(pattern.prefix(Self.maximumPatternLength))
    }

    private enum CodingKeys: String, CodingKey {
        case pattern
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        self.init(pattern: try container.decode(String.self, forKey: .pattern))
    }
}

public enum RuleDestinationMatchEvidence: Codable, Hashable, Sendable {
    case unconstrained
    case ip(IPAddress)
    case network(IPNetwork)
    case host(RuleHostDestinationEvidence)
    case hostPattern(RuleHostPatternDestinationEvidence)
}

public enum RuleProtocolMatchEvidence: Codable, Hashable, Sendable {
    case unconstrained
    case exact(TransportProtocol)
}

public enum RulePortMatchEvidence: Codable, Hashable, Sendable {
    case unconstrained
    case range(PortRange)
}

public enum RuleContextUnavailableReason: String, Codable, Hashable, Sendable {
    case missingSourceApplicationAuditToken
    case sourceIdentityResolutionFailed
    case sourceIdentityAuditTokenMismatch
    case sourceSigningIdentifierMismatch
    case emptyRemoteHost
    case invalidRemotePort
    case unsupportedRemoteEndpoint
}

public enum CaptureRuleEvaluationOutcome: String, Codable, Hashable, Sendable {
    case matchedRule
    case builtInBypass
    case defaultDirect
    case captureDisabled
    case configurationUnavailable
    case contextUnavailable
}

/// Structured evidence explaining why a capture decision was made.
///
/// Matcher fields contain only the selected, user-configured condition. Secret
/// identity material and payload data are never copied into this object. String
/// fields are capped before crossing the Network Extension control channel.
public struct CaptureRuleDecisionEvidence: Codable, Hashable, Sendable {
    public let outcome: CaptureRuleEvaluationOutcome
    public let source: RuleSourceMatchEvidence?
    public let destination: RuleDestinationMatchEvidence?
    public let transportProtocol: RuleProtocolMatchEvidence?
    public let destinationPort: RulePortMatchEvidence?
    public let builtInBypassReason: BuiltInBypassReason?
    public let contextUnavailableReason: RuleContextUnavailableReason?

    public init(
        outcome: CaptureRuleEvaluationOutcome,
        source: RuleSourceMatchEvidence? = nil,
        destination: RuleDestinationMatchEvidence? = nil,
        transportProtocol: RuleProtocolMatchEvidence? = nil,
        destinationPort: RulePortMatchEvidence? = nil,
        builtInBypassReason: BuiltInBypassReason? = nil,
        contextUnavailableReason: RuleContextUnavailableReason? = nil
    ) {
        self.outcome = outcome
        self.source = source
        self.destination = destination
        self.transportProtocol = transportProtocol
        self.destinationPort = destinationPort
        self.builtInBypassReason = builtInBypassReason
        self.contextUnavailableReason = contextUnavailableReason
    }
}

public struct RuleDecision: Codable, Hashable, Sendable {
    public let action: CaptureAction
    public let unavailableFallback: UnavailableFallback
    public let cause: RuleDecisionCause
    public let evidence: CaptureRuleDecisionEvidence?

    public init(
        action: CaptureAction,
        unavailableFallback: UnavailableFallback,
        cause: RuleDecisionCause,
        evidence: CaptureRuleDecisionEvidence? = nil
    ) {
        self.action = action
        self.unavailableFallback = unavailableFallback
        self.cause = cause
        self.evidence = evidence
    }
}

/// Security-critical bypasses are compiled into the engine and are deliberately
/// absent from user configuration snapshots.
public struct BuiltInBypassPolicy: Sendable {
    public init() {}

    public func reason(for context: FlowContext) -> BuiltInBypassReason? {
        if context.source.isTrustedMyproxyComponent {
            return .trustedmyproxyComponent
        }
        guard let address = context.destination.ipAddress else { return nil }
        if address.isLoopback { return .loopback }
        if address.isLinkLocal { return .linkLocal }
        if address.isMulticast { return .multicast }
        if address.isUnspecified { return .unspecifiedAddress }
        return nil
    }
}

public struct CaptureRuleEngine: Sendable {
    enum SourceEvaluationMode: Sendable {
        case verified
        case kernelMetadataOnly
        case unavailable
    }

    private struct IndexedDestination: Sendable {
        let insertionIndex: Int
        let matcher: DestinationMatcher
    }

    /// Compiles the common exact/suffix forms into lookup tables. Proxifier
    /// profiles commonly contain thousands of hostname masks in one rule;
    /// evaluating those as a linear wildcard list for every flow causes an
    /// avoidable CPU spike.
    private struct CompiledDestinations: Sendable {
        let exactIPs: [IPAddress: IndexedDestination]
        let networks: [IndexedDestination]
        let exactHosts: [String: IndexedDestination]
        let domainSuffixes: [String: IndexedDestination]
        let looseSuffixes: [String: IndexedDestination]
        let generalHostPatterns: [IndexedDestination]

        init(_ matchers: [DestinationMatcher]) {
            var exactIPs: [IPAddress: IndexedDestination] = [:]
            var networks: [IndexedDestination] = []
            var exactHosts: [String: IndexedDestination] = [:]
            var domainSuffixes: [String: IndexedDestination] = [:]
            var looseSuffixes: [String: IndexedDestination] = [:]
            var generalHostPatterns: [IndexedDestination] = []

            for (index, matcher) in matchers.enumerated() {
                let indexed = IndexedDestination(insertionIndex: index, matcher: matcher)
                switch matcher {
                case let .ip(address):
                    if exactIPs[address] == nil { exactIPs[address] = indexed }
                case .network:
                    networks.append(indexed)
                case let .host(host):
                    switch host.kind {
                    case .exact:
                        if exactHosts[host.value] == nil { exactHosts[host.value] = indexed }
                    case .suffix:
                        if domainSuffixes[host.value] == nil {
                            domainSuffixes[host.value] = indexed
                        }
                    }
                case let .hostPattern(pattern):
                    if let suffix = pattern.indexedLooseSuffix {
                        if looseSuffixes[suffix] == nil { looseSuffixes[suffix] = indexed }
                    } else {
                        generalHostPatterns.append(indexed)
                    }
                }
            }

            self.exactIPs = exactIPs
            self.networks = networks
            self.exactHosts = exactHosts
            self.domainSuffixes = domainSuffixes
            self.looseSuffixes = looseSuffixes
            self.generalHostPatterns = generalHostPatterns
        }

        func match(_ destination: FlowDestination) -> RuleDestinationMatchEvidence? {
            var best: IndexedDestination?

            func consider(_ candidate: IndexedDestination?) {
                guard let candidate else { return }
                if best == nil || candidate.insertionIndex < best!.insertionIndex {
                    best = candidate
                }
            }

            if let address = destination.ipAddress {
                consider(exactIPs[address])
                for indexed in networks {
                    guard case let .network(network) = indexed.matcher,
                          network.contains(address) else { continue }
                    consider(indexed)
                }
            }

            if let hostname = destination.hostname {
                let normalized = Self.normalizedHostname(hostname)
                consider(exactHosts[normalized])

                let labels = normalized.split(separator: ".", omittingEmptySubsequences: true)
                for index in labels.indices {
                    let suffix = labels[index...].joined(separator: ".")
                    consider(domainSuffixes[suffix])
                }

                let characters = Array(normalized)
                for index in characters.indices {
                    consider(looseSuffixes[String(characters[index...])])
                }

                for indexed in generalHostPatterns {
                    guard case let .hostPattern(pattern) = indexed.matcher,
                          pattern.matches(normalized) else { continue }
                    consider(indexed)
                }
            }

            guard let best else { return nil }
            return CaptureRuleEngine.destinationEvidence(for: best.matcher)
        }

        private static func normalizedHostname(_ hostname: String) -> String {
            var result = hostname.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
            while result.last == "." { result.removeLast() }
            return result
        }
    }

    private struct OrderedRule: Sendable {
        let rule: CaptureRule
        let insertionIndex: Int
        let destinations: CompiledDestinations
    }

    private let orderedRules: [OrderedRule]
    private let builtInBypass = BuiltInBypassPolicy()

    public init(snapshot: CaptureConfigurationSnapshot) {
        orderedRules = snapshot.rules.enumerated()
            .map {
                OrderedRule(
                    rule: $0.element,
                    insertionIndex: $0.offset,
                    destinations: CompiledDestinations($0.element.destinations)
                )
            }
            .sorted { lhs, rhs in
                if lhs.rule.priority != rhs.rule.priority {
                    return lhs.rule.priority < rhs.rule.priority
                }
                return lhs.insertionIndex < rhs.insertionIndex
            }
    }

    public func evaluate(_ context: FlowContext) -> RuleDecision {
        evaluate(context, sourceEvaluationMode: .verified)
    }

    func evaluate(
        _ context: FlowContext,
        sourceEvaluationMode: SourceEvaluationMode
    ) -> RuleDecision {
        if let reason = builtInBypass.reason(for: context) {
            return RuleDecision(
                action: .direct,
                unavailableFallback: .direct,
                cause: .builtInBypass(reason),
                evidence: CaptureRuleDecisionEvidence(
                    outcome: .builtInBypass,
                    builtInBypassReason: reason
                )
            )
        }

        for orderedRule in orderedRules where orderedRule.rule.enabled {
            let rule = orderedRule.rule
            if let evidence = Self.matchEvidence(
                for: rule,
                compiledDestinations: orderedRule.destinations,
                context: context,
                sourceEvaluationMode: sourceEvaluationMode
            ) {
                return RuleDecision(
                    action: rule.action,
                    unavailableFallback: rule.unavailableFallback,
                    cause: .matchedRule(rule.id),
                    evidence: evidence
                )
            }
        }
        return RuleDecision(
            action: .direct,
            unavailableFallback: .direct,
            cause: .defaultDirect,
            evidence: CaptureRuleDecisionEvidence(outcome: .defaultDirect)
        )
    }

    private static func matchEvidence(
        for rule: CaptureRule,
        compiledDestinations: CompiledDestinations,
        context: FlowContext,
        sourceEvaluationMode: SourceEvaluationMode
    ) -> CaptureRuleDecisionEvidence? {
        // Protocol matching is bounded and rejects many rules. Run it before
        // source identity, wildcard, network, and hostname matching so every
        // new flow pays the expensive work only for viable candidates.
        // Port ranges stay below those checks because an individual rule may
        // contain many ranges; moving that unbounded scan forward could regress
        // source-mismatch-heavy configurations.
        guard rule.protocols.isEmpty || rule.protocols.contains(context.transportProtocol) else {
            return nil
        }
        let protocolEvidence: RuleProtocolMatchEvidence = rule.protocols.isEmpty
            ? .unconstrained
            : .exact(context.transportProtocol)

        let sourceEvidence: RuleSourceMatchEvidence
        if rule.sources.isEmpty {
            sourceEvidence = .unconstrained
        } else if let evidence = rule.sources.lazy.compactMap({ matcher in
            matchEvidence(
                for: matcher,
                source: context.source,
                sourceEvaluationMode: sourceEvaluationMode
            )
        }).first {
            sourceEvidence = evidence
        } else {
            return nil
        }

        let destinationEvidence: RuleDestinationMatchEvidence
        if rule.destinations.isEmpty {
            destinationEvidence = .unconstrained
        } else if let evidence = compiledDestinations.match(context.destination) {
            destinationEvidence = evidence
        } else {
            return nil
        }

        let portEvidence: RulePortMatchEvidence
        if rule.portRanges.isEmpty {
            portEvidence = .unconstrained
        } else if let range = rule.portRanges.first(where: { $0.contains(context.destination.port) }) {
            portEvidence = .range(range)
        } else {
            return nil
        }

        return CaptureRuleDecisionEvidence(
            outcome: .matchedRule,
            source: sourceEvidence,
            destination: destinationEvidence,
            transportProtocol: protocolEvidence,
            destinationPort: portEvidence
        )
    }

    private static func matchEvidence(
        for matcher: SourceMatcher,
        source: FlowSource,
        sourceEvaluationMode: SourceEvaluationMode
    ) -> RuleSourceMatchEvidence? {
        switch sourceEvaluationMode {
        case .verified:
            break
        case .kernelMetadataOnly:
            // NEFlowMetaData's signing identifier remains usable even when
            // process inspection fails. This lets existing app selections and
            // identifier-pattern rules keep working without requiring a full
            // identity. Paths, PIDs and UIDs cannot be inferred from it.
            switch matcher {
            case .application, .applicationIdentifierPattern:
                break
            case .executable, .processInstance, .userID:
                return nil
            }
        case .unavailable:
            return nil
        }

        switch matcher {
        case let .application(application):
            if sourceEvaluationMode == .kernelMetadataOnly {
                guard let publishedIdentifier = source.signingIdentifier?
                    .trimmingCharacters(in: .whitespacesAndNewlines)
                    .lowercased(),
                    !publishedIdentifier.isEmpty
                else { return nil }
                let configuredIdentifiers = [
                    application.signingIdentifier,
                    application.bundleIdentifier,
                ].compactMap { value -> String? in
                    guard let value else { return nil }
                    let normalized = value
                        .trimmingCharacters(in: .whitespacesAndNewlines)
                        .lowercased()
                    return normalized.isEmpty ? nil : normalized
                }
                guard let matchedIdentifier = configuredIdentifiers.first(where: {
                    $0 == publishedIdentifier
                }) else { return nil }
                return .applicationIdentifierPattern(RuleApplicationPatternEvidence(
                    pattern: matchedIdentifier,
                    matchedField: .signingIdentifier
                ))
            }
            guard source.designatedRequirement == application.designatedRequirement else { return nil }
            if let expected = application.signingIdentifier, source.signingIdentifier != expected { return nil }
            if let expected = application.teamIdentifier, source.teamIdentifier != expected { return nil }
            if let expected = application.bundleIdentifier, source.bundleIdentifier != expected { return nil }
            return .application(RuleApplicationSourceEvidence(
                signingIdentifier: application.signingIdentifier,
                teamIdentifier: application.teamIdentifier,
                bundleIdentifier: application.bundleIdentifier
            ))
        case let .applicationIdentifierPattern(application):
            let candidates: [(String?, RuleApplicationIdentifierField)] = [
                (source.bundleIdentifier, .bundleIdentifier),
                (source.signingIdentifier, .signingIdentifier),
                (source.executablePath.map { URL(fileURLWithPath: $0).lastPathComponent }, .executableName),
            ]
            guard let matchedField = candidates.first(where: { candidate, _ in
                candidate.map(application.matches) == true
            })?.1 else {
                return nil
            }
            return .applicationIdentifierPattern(RuleApplicationPatternEvidence(
                pattern: application.pattern,
                matchedField: matchedField
            ))
        case let .executable(executable):
            guard source.executablePath == executable.canonicalPath else { return nil }
            if let expected = executable.designatedRequirement, source.designatedRequirement != expected { return nil }
            if let expected = executable.sha256?.lowercased(), source.executableSHA256?.lowercased() != expected {
                return nil
            }
            let assurance: RuleSourceIdentityAssurance = switch (
                executable.designatedRequirement != nil,
                executable.sha256 != nil
            ) {
            case (true, true): .exactExecutablePathCodeSignatureRequirementAndSHA256
            case (true, false): .exactExecutablePathAndCodeSignatureRequirement
            case (false, true): .exactExecutablePathAndSHA256
            case (false, false): .exactExecutablePath
            }
            return .executable(RuleExecutableSourceEvidence(
                canonicalPath: executable.canonicalPath,
                assurance: assurance
            ))
        case let .processInstance(process):
            guard source.processIdentifier == process.processIdentifier else { return nil }
            if let auditToken = process.auditToken {
                guard source.auditToken == auditToken else { return nil }
                return .processInstance(RuleProcessInstanceSourceEvidence(
                    processIdentifier: process.processIdentifier,
                    canonicalExecutablePath: nil,
                    assurance: .exactAuditToken
                ))
            }
            guard source.processStartTime == process.startTime else { return nil }
            if let path = process.canonicalExecutablePath {
                guard source.executablePath == path else { return nil }
                return .processInstance(RuleProcessInstanceSourceEvidence(
                    processIdentifier: process.processIdentifier,
                    canonicalExecutablePath: path,
                    assurance: .exactProcessStartTimeAndExecutablePath
                ))
            }
            return nil
        case let .userID(userID):
            guard source.userID == userID else { return nil }
            return .userID(userID)
        }
    }

    private static func matchEvidence(
        for matcher: DestinationMatcher,
        destination: FlowDestination
    ) -> RuleDestinationMatchEvidence? {
        switch matcher {
        case let .ip(address):
            return destination.ipAddress == address ? .ip(address) : nil
        case let .network(network):
            guard let address = destination.ipAddress, network.contains(address) else { return nil }
            return .network(network)
        case let .host(host):
            guard let hostname = destination.hostname, host.matches(hostname) else { return nil }
            return .host(RuleHostDestinationEvidence(kind: host.kind, value: host.value))
        case let .hostPattern(pattern):
            guard let hostname = destination.hostname, pattern.matches(hostname) else { return nil }
            return .hostPattern(RuleHostPatternDestinationEvidence(pattern: pattern.pattern))
        }
    }

    private static func destinationEvidence(
        for matcher: DestinationMatcher
    ) -> RuleDestinationMatchEvidence {
        switch matcher {
        case let .ip(address):
            .ip(address)
        case let .network(network):
            .network(network)
        case let .host(host):
            .host(RuleHostDestinationEvidence(kind: host.kind, value: host.value))
        case let .hostPattern(pattern):
            .hostPattern(RuleHostPatternDestinationEvidence(pattern: pattern.pattern))
        }
    }
}
