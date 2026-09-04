import Foundation

public enum DNSProxyRuntimePhase: String, Codable, CaseIterable, Sendable {
    case starting
    case running
    case stopping
    case stopped
    case failed
}

/// Coarse, privacy-safe failure information suitable for crossing the process
/// boundary. Free-form error text is intentionally excluded so a destination,
/// DNS question, payload, or credential cannot accidentally be persisted.
public enum DNSProxyFailureCategory: String, Codable, CaseIterable, Sendable {
    case invalidConfiguration
    case sharedContainerUnavailable
    case backendUnavailable
    case tcpRelayFailed
    case udpRelayFailed
    case flowConversionFailed
    case statusPersistenceFailed
    case cancelled
    case unknown
}

/// Privacy-safe reasons that can be published even when the DNS provider
/// cannot decode enough bootstrap state to construct a runtime status value.
public enum DNSProxyStartupFailureReason: String, Codable, CaseIterable, Sendable {
    case missingProviderConfiguration
    case missingBootstrapPayload
    case invalidBootstrapPayload
    case invalidPrivateRelay
}

public struct DNSProxyStartupFailure: Codable, Equatable, Sendable {
    public let reason: DNSProxyStartupFailureReason
    public let observedAt: Date

    public init(reason: DNSProxyStartupFailureReason, observedAt: Date = Date()) {
        self.reason = reason
        self.observedAt = observedAt
    }
}

/// Snapshot returned through the already-authenticated transparent-provider
/// message channel. The expectation is installed before NEDNSProxyManager is
/// enabled, so even a pre-bootstrap failure is tied to one activation.
public struct DNSProxyRuntimeReport: Codable, Equatable, Sendable {
    public let expectedRevision: UInt64
    public let expectedActivationIdentifier: UUID
    public let status: DNSProxyRuntimeStatus?
    public let startupFailure: DNSProxyStartupFailure?

    public init(
        expectedRevision: UInt64,
        expectedActivationIdentifier: UUID,
        status: DNSProxyRuntimeStatus? = nil,
        startupFailure: DNSProxyStartupFailure? = nil
    ) {
        self.expectedRevision = expectedRevision
        self.expectedActivationIdentifier = expectedActivationIdentifier
        self.status = status
        self.startupFailure = startupFailure
    }
}

/// Bounded operational truth published by the DNS provider.
///
/// The model contains only lifecycle, aggregate counters, and timestamps. It
/// must never grow fields for query names, destination endpoints, packet
/// payloads, process identity, or proxy credentials.
public struct DNSProxyRuntimeStatus: Codable, Equatable, Sendable {
    public static let currentSchemaVersion = 2
    public static let defaultMaximumHeartbeatAge: TimeInterval = 9

    public let schemaVersion: Int
    public let revision: UInt64
    public let activationIdentifier: UUID
    public let providerInstanceIdentifier: UUID

    public var phase: DNSProxyRuntimePhase
    public var backendReady: Bool
    public var activeTCPFlows: UInt64
    public var activeUDPFlows: UInt64
    public var totalFlows: UInt64
    public var completedFlows: UInt64
    public var failedFlows: UInt64
    public var uploadBytes: UInt64
    public var downloadBytes: UInt64

    public let startedAt: Date
    /// The provider heartbeat timestamp. Hosts use this field for freshness,
    /// even when no DNS flow is currently active.
    public var updatedAt: Date
    /// Latest successful private SOCKS5 UDP association probe. This proves
    /// reachability of the Mihomo listener, not that a DNS answer was received.
    public var lastBackendAssociationAt: Date?
    /// Latest application payload accepted by the private relay.
    public var lastQueryForwardedAt: Date?
    /// Latest response payload delivered back to a DNS client.
    public var lastResponseDeliveredAt: Date?
    public var lastFailureAt: Date?
    public var failureCategory: DNSProxyFailureCategory?

    public init(
        revision: UInt64,
        activationIdentifier: UUID,
        providerInstanceIdentifier: UUID = UUID(),
        phase: DNSProxyRuntimePhase,
        backendReady: Bool,
        activeTCPFlows: UInt64 = 0,
        activeUDPFlows: UInt64 = 0,
        totalFlows: UInt64 = 0,
        completedFlows: UInt64 = 0,
        failedFlows: UInt64 = 0,
        uploadBytes: UInt64 = 0,
        downloadBytes: UInt64 = 0,
        startedAt: Date,
        updatedAt: Date? = nil,
        lastBackendAssociationAt: Date? = nil,
        lastQueryForwardedAt: Date? = nil,
        lastResponseDeliveredAt: Date? = nil,
        lastFailureAt: Date? = nil,
        failureCategory: DNSProxyFailureCategory? = nil
    ) {
        schemaVersion = Self.currentSchemaVersion
        self.revision = revision
        self.activationIdentifier = activationIdentifier
        self.providerInstanceIdentifier = providerInstanceIdentifier
        self.phase = phase
        self.backendReady = backendReady
        self.activeTCPFlows = activeTCPFlows
        self.activeUDPFlows = activeUDPFlows
        self.totalFlows = totalFlows
        self.completedFlows = completedFlows
        self.failedFlows = failedFlows
        self.uploadBytes = uploadBytes
        self.downloadBytes = downloadBytes
        self.startedAt = startedAt
        self.updatedAt = updatedAt ?? startedAt
        self.lastBackendAssociationAt = lastBackendAssociationAt
        self.lastQueryForwardedAt = lastQueryForwardedAt
        self.lastResponseDeliveredAt = lastResponseDeliveredAt
        self.lastFailureAt = lastFailureAt
        self.failureCategory = failureCategory
    }

    public var activeFlows: UInt64 {
        let (value, overflow) = activeTCPFlows.addingReportingOverflow(activeUDPFlows)
        return overflow ? .max : value
    }

    public var isOperational: Bool {
        phase == .running && backendReady
    }

    public mutating func recordHeartbeat(at date: Date = Date()) {
        updatedAt = date
    }

    public func isFresh(
        at date: Date = Date(),
        maximumAge: TimeInterval = Self.defaultMaximumHeartbeatAge
    ) -> Bool {
        guard maximumAge.isFinite, maximumAge >= 0 else { return false }
        return date.timeIntervalSince(updatedAt) <= maximumAge
    }

    /// Validates both document invariants and the activation-specific runtime
    /// expectation held by the host. A matching revision alone is insufficient
    /// because an old provider process can publish the same saved revision.
    public func validate(
        expectedRevision: UInt64,
        activationIdentifier expectedActivationIdentifier: UUID,
        at date: Date = Date(),
        maximumAge: TimeInterval = Self.defaultMaximumHeartbeatAge
    ) throws {
        try validate()
        guard revision == expectedRevision else {
            throw DNSProxyRuntimeStatusValidationError.revisionMismatch(
                expected: expectedRevision,
                actual: revision
            )
        }
        guard activationIdentifier == expectedActivationIdentifier else {
            throw DNSProxyRuntimeStatusValidationError.activationMismatch(
                expected: expectedActivationIdentifier,
                actual: activationIdentifier
            )
        }
        guard maximumAge.isFinite, maximumAge >= 0 else {
            throw DNSProxyRuntimeStatusValidationError.invalidMaximumHeartbeatAge
        }
        guard isFresh(at: date, maximumAge: maximumAge) else {
            throw DNSProxyRuntimeStatusValidationError.staleHeartbeat(
                updatedAt: updatedAt,
                evaluatedAt: date,
                maximumAge: maximumAge
            )
        }
    }

    public func validate() throws {
        guard schemaVersion == Self.currentSchemaVersion else {
            throw DNSProxyRuntimeStatusValidationError.unsupportedSchemaVersion(
                schemaVersion
            )
        }
        guard revision > 0 else {
            throw DNSProxyRuntimeStatusValidationError.invalidRevision(revision)
        }

        let (activeFlowCount, activeOverflow) = activeTCPFlows.addingReportingOverflow(
            activeUDPFlows
        )
        guard !activeOverflow, activeFlowCount <= totalFlows else {
            throw DNSProxyRuntimeStatusValidationError.flowCountInvariantViolation
        }
        let (terminalFlowCount, terminalOverflow) = completedFlows.addingReportingOverflow(
            failedFlows
        )
        guard !terminalOverflow else {
            throw DNSProxyRuntimeStatusValidationError.flowCountInvariantViolation
        }
        let (accountedFlowCount, accountedOverflow) = terminalFlowCount
            .addingReportingOverflow(activeFlowCount)
        guard !accountedOverflow, accountedFlowCount <= totalFlows else {
            throw DNSProxyRuntimeStatusValidationError.flowCountInvariantViolation
        }

        guard updatedAt >= startedAt else {
            throw DNSProxyRuntimeStatusValidationError.updatedBeforeStart
        }
        for event in [
            lastBackendAssociationAt,
            lastQueryForwardedAt,
            lastResponseDeliveredAt,
        ].compactMap({ $0 }) where event > updatedAt {
            throw DNSProxyRuntimeStatusValidationError.eventAfterHeartbeat
        }
        if let lastFailureAt, lastFailureAt > updatedAt {
            throw DNSProxyRuntimeStatusValidationError.eventAfterHeartbeat
        }
        for event in [
            lastBackendAssociationAt,
            lastQueryForwardedAt,
            lastResponseDeliveredAt,
        ].compactMap({ $0 }) where event < startedAt {
            throw DNSProxyRuntimeStatusValidationError.eventBeforeStart
        }
        if let lastFailureAt, lastFailureAt < startedAt {
            throw DNSProxyRuntimeStatusValidationError.eventBeforeStart
        }
        guard (lastFailureAt == nil) == (failureCategory == nil) else {
            throw DNSProxyRuntimeStatusValidationError.incompleteFailureRecord
        }

        if phase == .failed || (phase == .running && !backendReady) {
            guard failureCategory != nil else {
                throw DNSProxyRuntimeStatusValidationError.missingFailureCategory
            }
        }
        if phase == .stopped || phase == .failed {
            guard !backendReady else {
                throw DNSProxyRuntimeStatusValidationError.terminalPhaseBackendReady
            }
        }
        if phase == .stopped {
            guard activeFlowCount == 0 else {
                throw DNSProxyRuntimeStatusValidationError.stoppedWithActiveFlows
            }
        }
    }
}

public enum DNSProxyRuntimeStatusValidationError: Error, Equatable, Sendable {
    case unsupportedSchemaVersion(Int)
    case invalidRevision(UInt64)
    case flowCountInvariantViolation
    case updatedBeforeStart
    case eventBeforeStart
    case eventAfterHeartbeat
    case incompleteFailureRecord
    case missingFailureCategory
    case terminalPhaseBackendReady
    case stoppedWithActiveFlows
    case revisionMismatch(expected: UInt64, actual: UInt64)
    case activationMismatch(expected: UUID, actual: UUID)
    case invalidMaximumHeartbeatAge
    case staleHeartbeat(updatedAt: Date, evaluatedAt: Date, maximumAge: TimeInterval)
}

extension DNSProxyRuntimeStatusValidationError: LocalizedError {
    public var errorDescription: String? {
        switch self {
        case let .unsupportedSchemaVersion(version):
            "DNS proxy status uses unsupported schema version \(version)."
        case let .invalidRevision(revision):
            "DNS proxy status revision must be greater than zero; received \(revision)."
        case .flowCountInvariantViolation:
            "DNS proxy status flow counters are inconsistent."
        case .updatedBeforeStart:
            "DNS proxy status heartbeat predates provider startup."
        case .eventBeforeStart:
            "DNS proxy status contains an event older than provider startup."
        case .eventAfterHeartbeat:
            "DNS proxy status contains an event newer than its heartbeat."
        case .incompleteFailureRecord:
            "DNS proxy status must record failure time and category together."
        case .missingFailureCategory:
            "DNS proxy status does not identify its runtime failure category."
        case .terminalPhaseBackendReady:
            "A stopped or failed DNS proxy cannot report its backend as ready."
        case .stoppedWithActiveFlows:
            "A stopped DNS proxy cannot report active flows."
        case let .revisionMismatch(expected, actual):
            "DNS proxy status revision \(actual) does not match expected revision \(expected)."
        case .activationMismatch:
            "DNS proxy status was published by a different activation."
        case .invalidMaximumHeartbeatAge:
            "DNS proxy heartbeat maximum age must be a finite, nonnegative interval."
        case let .staleHeartbeat(updatedAt, evaluatedAt, maximumAge):
            "DNS proxy heartbeat from \(updatedAt) is older than \(maximumAge) seconds at \(evaluatedAt)."
        }
    }
}
