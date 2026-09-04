import Foundation

public enum TransportProtocol: String, Codable, CaseIterable, Hashable, Sendable {
    case tcp
    case udp
}

public struct ApplicationSourceMatcher: Codable, Hashable, Sendable {
    public let designatedRequirement: String
    public let signingIdentifier: String?
    public let teamIdentifier: String?
    public let bundleIdentifier: String?

    public init(
        designatedRequirement: String,
        signingIdentifier: String? = nil,
        teamIdentifier: String? = nil,
        bundleIdentifier: String? = nil
    ) {
        self.designatedRequirement = designatedRequirement
        self.signingIdentifier = signingIdentifier
        self.teamIdentifier = teamIdentifier
        self.bundleIdentifier = bundleIdentifier
    }
}

public struct ApplicationIdentifierPatternMatcher: Codable, Hashable, Sendable {
    public let pattern: String

    public init(pattern: String) throws {
        let normalized = pattern.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        let allowedPunctuation: Set<Character> = [".", "-", "_", "*", "?", " "]
        guard !normalized.isEmpty,
              normalized.utf8.count <= 255,
              normalized.contains(where: { $0 != "*" && $0 != "?" && $0 != " " }),
              normalized.allSatisfy({ $0.isLetter || $0.isNumber || allowedPunctuation.contains($0) })
        else {
            throw NetworkRuleValidationError.invalidSourceMatcher(
                "Application pattern must contain a name or identifier and may use * or ? wildcards"
            )
        }
        self.pattern = normalized
    }

    public func matches(_ candidate: String) -> Bool {
        let candidate = candidate.lowercased()
        guard pattern.contains("*") || pattern.contains("?") else {
            return pattern.elementsEqual(candidate)
        }
        return Self.wildcardMatch(pattern: Array(pattern), candidate: Array(candidate))
    }

    private static func wildcardMatch(pattern: [Character], candidate: [Character]) -> Bool {
        var patternIndex = 0
        var candidateIndex = 0
        var starIndex: Int?
        var starCandidateIndex = 0

        while candidateIndex < candidate.count {
            if patternIndex < pattern.count,
               pattern[patternIndex] == "?" || pattern[patternIndex] == candidate[candidateIndex] {
                patternIndex += 1
                candidateIndex += 1
            } else if patternIndex < pattern.count, pattern[patternIndex] == "*" {
                starIndex = patternIndex
                patternIndex += 1
                starCandidateIndex = candidateIndex
            } else if let starIndex {
                patternIndex = starIndex + 1
                starCandidateIndex += 1
                candidateIndex = starCandidateIndex
            } else {
                return false
            }
        }

        while patternIndex < pattern.count, pattern[patternIndex] == "*" {
            patternIndex += 1
        }
        return patternIndex == pattern.count
    }

    private enum CodingKeys: String, CodingKey {
        case pattern
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let pattern = try container.decode(String.self, forKey: .pattern)
        do {
            try self.init(pattern: pattern)
        } catch {
            throw DecodingError.dataCorruptedError(
                forKey: .pattern,
                in: container,
                debugDescription: String(describing: error)
            )
        }
    }
}

public struct ExecutableSourceMatcher: Codable, Hashable, Sendable {
    public let canonicalPath: String
    public let designatedRequirement: String?
    public let sha256: String?

    public init(canonicalPath: String, designatedRequirement: String? = nil, sha256: String? = nil) {
        self.canonicalPath = canonicalPath
        self.designatedRequirement = designatedRequirement
        self.sha256 = sha256?.lowercased()
    }
}

public struct ProcessInstanceSourceMatcher: Codable, Hashable, Sendable {
    public let processIdentifier: Int32
    public let auditToken: Data?
    public let startTime: ProcessStartTime?
    public let canonicalExecutablePath: String?

    public init(processIdentifier: Int32, auditToken: Data) {
        self.processIdentifier = processIdentifier
        self.auditToken = auditToken
        startTime = nil
        canonicalExecutablePath = nil
    }

    public init(
        processIdentifier: Int32,
        startTime: ProcessStartTime,
        canonicalExecutablePath: String
    ) {
        self.processIdentifier = processIdentifier
        auditToken = nil
        self.startTime = startTime
        self.canonicalExecutablePath = canonicalExecutablePath
    }
}

/// Public `libproc` start timestamp used with a PID to identify one running
/// process instance without requiring the Endpoint Security entitlement.
public struct ProcessStartTime: Codable, Hashable, Sendable {
    public let seconds: UInt64
    public let microseconds: UInt32

    public init(seconds: UInt64, microseconds: UInt32) throws {
        guard microseconds < 1_000_000 else {
            throw NetworkRuleValidationError.invalidSourceMatcher(
                "Process start-time microseconds must be below 1000000"
            )
        }
        self.seconds = seconds
        self.microseconds = microseconds
    }
}

public enum SourceMatcher: Codable, Hashable, Sendable {
    case application(ApplicationSourceMatcher)
    case applicationIdentifierPattern(ApplicationIdentifierPatternMatcher)
    case executable(ExecutableSourceMatcher)
    case processInstance(ProcessInstanceSourceMatcher)
    case userID(UInt32)
}

public struct HostMatcher: Codable, Hashable, Sendable {
    public enum Kind: String, Codable, Hashable, Sendable {
        case exact
        case suffix
    }

    public let kind: Kind
    public let value: String

    public init(kind: Kind, value: String) throws {
        let normalized = Self.normalize(value)
        guard Self.isValid(normalized) else {
            throw NetworkRuleValidationError.invalidDomain(value)
        }
        self.kind = kind
        self.value = normalized
    }

    public func matches(_ hostname: String) -> Bool {
        let candidate = Self.normalize(hostname)
        switch kind {
        case .exact:
            return candidate == value
        case .suffix:
            return candidate == value || candidate.hasSuffix(".\(value)")
        }
    }

    private static func normalize(_ value: String) -> String {
        var result = value.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        while result.last == "." {
            result.removeLast()
        }
        return result
    }

    private static func isValid(_ value: String) -> Bool {
        guard !value.isEmpty, value.utf8.count <= 253 else { return false }
        let labels = value.split(separator: ".", omittingEmptySubsequences: false)
        return labels.allSatisfy { label in
            guard !label.isEmpty, label.utf8.count <= 63,
                  label.first != "-", label.last != "-"
            else { return false }
            return label.allSatisfy { character in
                character.isLetter || character.isNumber || character == "-"
            }
        }
    }

    private enum CodingKeys: String, CodingKey {
        case kind
        case value
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let kind = try container.decode(Kind.self, forKey: .kind)
        let value = try container.decode(String.self, forKey: .value)
        do {
            try self.init(kind: kind, value: value)
        } catch {
            throw DecodingError.dataCorruptedError(
                forKey: .value,
                in: container,
                debugDescription: String(describing: error)
            )
        }
    }
}

/// A Proxifier-compatible hostname mask. This is deliberately separate from
/// `HostMatcher`: exact/suffix rules keep their domain-boundary semantics,
/// while imported masks such as `*github*` retain their wildcard behavior.
public struct HostPatternMatcher: Codable, Hashable, Sendable {
    public let pattern: String

    public init(pattern: String) throws {
        let normalized = pattern.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        let allowedPunctuation: Set<Character> = [".", "-", "_", "*", "?"]
        guard !normalized.isEmpty,
              normalized.utf8.count <= 253,
              normalized.contains(where: { $0 != "*" && $0 != "?" }),
              normalized.allSatisfy({
                  $0.isLetter || $0.isNumber || allowedPunctuation.contains($0)
              })
        else {
            throw NetworkRuleValidationError.invalidDomain(pattern)
        }
        self.pattern = normalized
    }

    public func matches(_ hostname: String) -> Bool {
        let candidate = hostname
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .lowercased()
            .trimmingCharacters(in: CharacterSet(charactersIn: "."))
        return Self.wildcardMatch(pattern: Array(pattern), candidate: Array(candidate))
    }

    /// Returns the literal suffix for the common `*example.com` form. The
    /// engine indexes these masks so large imported domain lists do not
    /// perform thousands of wildcard scans for every flow.
    public var indexedLooseSuffix: String? {
        guard pattern.first == "*" else { return nil }
        let suffix = String(pattern.dropFirst())
        guard !suffix.isEmpty,
              !suffix.contains("*"),
              !suffix.contains("?") else { return nil }
        return suffix
    }

    private static func wildcardMatch(pattern: [Character], candidate: [Character]) -> Bool {
        var patternIndex = 0
        var candidateIndex = 0
        var starIndex: Int?
        var starCandidateIndex = 0

        while candidateIndex < candidate.count {
            if patternIndex < pattern.count,
               pattern[patternIndex] == "?" || pattern[patternIndex] == candidate[candidateIndex] {
                patternIndex += 1
                candidateIndex += 1
            } else if patternIndex < pattern.count, pattern[patternIndex] == "*" {
                starIndex = patternIndex
                patternIndex += 1
                starCandidateIndex = candidateIndex
            } else if let starIndex {
                patternIndex = starIndex + 1
                starCandidateIndex += 1
                candidateIndex = starCandidateIndex
            } else {
                return false
            }
        }

        while patternIndex < pattern.count, pattern[patternIndex] == "*" {
            patternIndex += 1
        }
        return patternIndex == pattern.count
    }

    private enum CodingKeys: String, CodingKey {
        case pattern
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let pattern = try container.decode(String.self, forKey: .pattern)
        do {
            try self.init(pattern: pattern)
        } catch {
            throw DecodingError.dataCorruptedError(
                forKey: .pattern,
                in: container,
                debugDescription: String(describing: error)
            )
        }
    }
}

public enum DestinationMatcher: Codable, Hashable, Sendable {
    case ip(IPAddress)
    case network(IPNetwork)
    case host(HostMatcher)
    case hostPattern(HostPatternMatcher)
}

/// The routing mode requested within one explicitly identified profile.
public enum MihomoProfileRoute: Codable, Hashable, Sendable {
    case rules
    case global
    case group(String)
}

public enum MihomoRoute: Codable, Hashable, Sendable {
    /// Legacy targets resolve against the app's active/default profile.
    case profileRules
    case global
    case group(String)
    /// A complete multi-profile target. Both the profile and its routing mode
    /// participate in equality and hashing, so listener lookup is exact.
    case profile(RoutingProfileID, target: MihomoProfileRoute)

    public var routingProfileID: RoutingProfileID? {
        guard case let .profile(profileID, _) = self else { return nil }
        return profileID
    }

    public var profileRoute: MihomoProfileRoute {
        switch self {
        case .profileRules:
            .rules
        case .global:
            .global
        case let .group(name):
            .group(name)
        case let .profile(_, route):
            route
        }
    }

    /// Stable ordering for listener construction and diagnostics. Legacy
    /// routes remain first in their historical rules/global/group order.
    public var stableSortKey: String {
        switch self {
        case .profileRules:
            "0:legacy:0:rules"
        case .global:
            "0:legacy:1:global"
        case let .group(name):
            "0:legacy:2:group:\(name)"
        case let .profile(profileID, route):
            "1:\(profileID.rawValue):\(route.stableSortKey)"
        }
    }
}

private extension MihomoProfileRoute {
    var stableSortKey: String {
        switch self {
        case .rules:
            "0:rules"
        case .global:
            "1:global"
        case let .group(name):
            "2:group:\(name)"
        }
    }
}

public enum CaptureAction: Codable, Hashable, Sendable {
    case direct
    case reject
    case mihomo(MihomoRoute)
}

public enum UnavailableFallback: String, Codable, Hashable, Sendable {
    case direct
    case reject
}

public struct CaptureRule: Codable, Hashable, Identifiable, Sendable {
    public let id: String
    public let enabled: Bool
    public let priority: Int
    public let sources: [SourceMatcher]
    public let destinations: [DestinationMatcher]
    public let protocols: Set<TransportProtocol>
    public let portRanges: [PortRange]
    public let action: CaptureAction
    public let unavailableFallback: UnavailableFallback

    public init(
        id: String,
        enabled: Bool = true,
        priority: Int,
        sources: [SourceMatcher] = [],
        destinations: [DestinationMatcher] = [],
        protocols: Set<TransportProtocol> = [],
        portRanges: [PortRange] = [],
        action: CaptureAction,
        unavailableFallback: UnavailableFallback = .direct
    ) throws {
        self.id = id
        self.enabled = enabled
        self.priority = priority
        self.sources = sources
        self.destinations = destinations
        self.protocols = protocols
        self.portRanges = portRanges
        self.action = action
        self.unavailableFallback = unavailableFallback
        try validate()
    }

    public func validate() throws {
        guard !id.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw NetworkRuleValidationError.invalidRuleIdentifier(id)
        }
        for source in sources {
            switch source {
            case let .application(application):
                guard !application.designatedRequirement.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
                    throw NetworkRuleValidationError.invalidSourceMatcher(
                        "Application matcher requires a designated requirement"
                    )
                }
            case let .applicationIdentifierPattern(application):
                _ = try ApplicationIdentifierPatternMatcher(pattern: application.pattern)
            case let .executable(executable):
                guard executable.canonicalPath.hasPrefix("/"), executable.canonicalPath.count > 1 else {
                    throw NetworkRuleValidationError.invalidSourceMatcher(
                        "Executable matcher requires an absolute canonical path"
                    )
                }
            case let .processInstance(process):
                let hasValidIdentity: Bool
                if let auditToken = process.auditToken {
                    hasValidIdentity = auditToken.count == 32
                        && process.startTime == nil
                        && process.canonicalExecutablePath == nil
                } else if let path = process.canonicalExecutablePath {
                    hasValidIdentity = process.startTime != nil
                        && path.hasPrefix("/")
                        && path.count > 1
                        && !path.contains(where: { $0 == "\0" || $0 == "\n" || $0 == "\r" })
                } else {
                    hasValidIdentity = false
                }
                guard process.processIdentifier > 0, hasValidIdentity else {
                    throw NetworkRuleValidationError.invalidSourceMatcher(
                        "Process matcher requires a positive PID and exactly one audit-token or start-time/path identity"
                    )
                }
            case .userID:
                break
            }
        }
        if case let .mihomo(route) = action {
            let group: String? = switch route {
            case let .group(value):
                value
            case let .profile(_, target: .group(value)):
                value
            case .profileRules, .global, .profile:
                nil
            }
            if let group,
               group.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                throw NetworkRuleValidationError.invalidMihomoGroup(group)
            }
        }
    }

    private enum CodingKeys: String, CodingKey {
        case id
        case enabled
        case priority
        case sources
        case destinations
        case protocols
        case portRanges
        case action
        case unavailableFallback
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decode(String.self, forKey: .id)
        enabled = try container.decode(Bool.self, forKey: .enabled)
        priority = try container.decode(Int.self, forKey: .priority)
        sources = try container.decode([SourceMatcher].self, forKey: .sources)
        destinations = try container.decode([DestinationMatcher].self, forKey: .destinations)
        protocols = try container.decode(Set<TransportProtocol>.self, forKey: .protocols)
        portRanges = try container.decode([PortRange].self, forKey: .portRanges)
        action = try container.decode(CaptureAction.self, forKey: .action)
        unavailableFallback = try container.decode(UnavailableFallback.self, forKey: .unavailableFallback)
        do {
            try validate()
        } catch {
            throw DecodingError.dataCorruptedError(
                forKey: .id,
                in: container,
                debugDescription: String(describing: error)
            )
        }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(id, forKey: .id)
        try container.encode(enabled, forKey: .enabled)
        try container.encode(priority, forKey: .priority)
        try container.encode(sources, forKey: .sources)
        try container.encode(destinations, forKey: .destinations)
        try container.encode(protocols.sorted { $0.rawValue < $1.rawValue }, forKey: .protocols)
        try container.encode(portRanges, forKey: .portRanges)
        try container.encode(action, forKey: .action)
        try container.encode(unavailableFallback, forKey: .unavailableFallback)
    }
}

public struct FlowSource: Codable, Hashable, Sendable {
    public let processIdentifier: Int32
    public let auditToken: Data
    public let processStartTime: ProcessStartTime?
    /// Kernel-supplied, per-build identifier from `NEFlowMetaData`. An empty
    /// identifier for system processes is represented as `nil`.
    public let sourceAppUniqueIdentifier: Data?
    public let userID: UInt32
    public let executablePath: String?
    public let executableSHA256: String?
    public let designatedRequirement: String?
    public let signingIdentifier: String?
    public let teamIdentifier: String?
    public let bundleIdentifier: String?
    public let isTrustedMyproxyComponent: Bool

    public init(
        processIdentifier: Int32,
        auditToken: Data,
        processStartTime: ProcessStartTime? = nil,
        sourceAppUniqueIdentifier: Data? = nil,
        userID: UInt32,
        executablePath: String? = nil,
        executableSHA256: String? = nil,
        designatedRequirement: String? = nil,
        signingIdentifier: String? = nil,
        teamIdentifier: String? = nil,
        bundleIdentifier: String? = nil,
        isTrustedMyproxyComponent: Bool = false
    ) {
        self.processIdentifier = processIdentifier
        self.auditToken = auditToken
        self.processStartTime = processStartTime
        self.sourceAppUniqueIdentifier = sourceAppUniqueIdentifier
        self.userID = userID
        self.executablePath = executablePath
        self.executableSHA256 = executableSHA256?.lowercased()
        self.designatedRequirement = designatedRequirement
        self.signingIdentifier = signingIdentifier
        self.teamIdentifier = teamIdentifier
        self.bundleIdentifier = bundleIdentifier
        self.isTrustedMyproxyComponent = isTrustedMyproxyComponent
    }
}

public struct FlowDestination: Codable, Hashable, Sendable {
    public let hostname: String?
    public let ipAddress: IPAddress?
    public let port: UInt16

    public init(hostname: String? = nil, ipAddress: IPAddress? = nil, port: UInt16) throws {
        guard port > 0 else {
            throw NetworkRuleValidationError.invalidDestinationPort(port)
        }
        self.hostname = hostname
        self.ipAddress = ipAddress
        self.port = port
    }

    private enum CodingKeys: String, CodingKey {
        case hostname
        case ipAddress
        case port
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let hostname = try container.decodeIfPresent(String.self, forKey: .hostname)
        let ipAddress = try container.decodeIfPresent(IPAddress.self, forKey: .ipAddress)
        let port = try container.decode(UInt16.self, forKey: .port)
        do {
            try self.init(hostname: hostname, ipAddress: ipAddress, port: port)
        } catch {
            throw DecodingError.dataCorruptedError(
                forKey: .port,
                in: container,
                debugDescription: String(describing: error)
            )
        }
    }
}

public struct FlowContext: Codable, Hashable, Sendable {
    public let source: FlowSource
    public let destination: FlowDestination
    public let transportProtocol: TransportProtocol

    public init(source: FlowSource, destination: FlowDestination, transportProtocol: TransportProtocol) {
        self.source = source
        self.destination = destination
        self.transportProtocol = transportProtocol
    }
}
