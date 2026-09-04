import Foundation

/// A privacy-bounded description of the process that originated an App Routing flow.
///
/// Audit tokens, listener credentials, and designated requirements are deliberately
/// absent. Those values are useful while making a trusted decision inside the
/// Network Extension, but the activity channel does not need to export them.
public struct AppRoutingActivitySource: Codable, Hashable, Sendable {
    public let processIdentifier: Int32
    public let processStartTime: ProcessStartTime?
    public let userIdentifier: UInt32
    public let executablePath: String?
    public let bundleIdentifier: String?
    public let signingIdentifier: String?
    public let teamIdentifier: String?

    public init(
        processIdentifier: Int32,
        processStartTime: ProcessStartTime? = nil,
        userIdentifier: UInt32,
        executablePath: String? = nil,
        bundleIdentifier: String? = nil,
        signingIdentifier: String? = nil,
        teamIdentifier: String? = nil
    ) {
        self.processIdentifier = processIdentifier
        self.processStartTime = processStartTime
        self.userIdentifier = userIdentifier
        self.executablePath = executablePath
        self.bundleIdentifier = bundleIdentifier
        self.signingIdentifier = signingIdentifier
        self.teamIdentifier = teamIdentifier
    }
}

public struct AppRoutingActivityDestination: Codable, Hashable, Sendable {
    public let hostname: String?
    public let ipAddress: String?
    public let port: UInt16

    public init(
        hostname: String? = nil,
        ipAddress: String? = nil,
        port: UInt16
    ) {
        self.hostname = hostname
        self.ipAddress = ipAddress
        self.port = port
    }
}

public enum AppRoutingRelayState: String, Codable, Hashable, Sendable {
    /// Rejected, normal Direct, built-in bypass, and fail-open decisions do not
    /// create a measurable relay. A Direct fallback inside an already-owned
    /// Mihomo flow uses the normal relay lifecycle states.
    case notApplicable
    case pending
    case connecting
    case ready
    case relaying
    case completed
    case failed
}

/// Identifies the Network Extension data plane that observed a flow.
///
/// This field was added after App Routing activity persistence shipped, so
/// consumers must continue to treat a missing value as ordinary App Routing.
public enum AppRoutingActivityCaptureOrigin: String, Codable, Hashable, Sendable {
    case appRouting
    case dnsProxy
}

/// The latest observable state for one flow handled by App Routing.
///
/// `decision` preserves the rule engine's original result. `configuredAction`
/// records what the matching rule requested, while `effectiveAction` records
/// what the provider actually did after availability checks and fallbacks. The
/// distinction prevents a requested Mihomo route that fell back to direct from
/// being presented as proxied traffic.
public struct AppRoutingActivity: Codable, Hashable, Sendable, Identifiable {
    public let flowIdentifier: UUID
    /// Groups per-destination UDP conversations that originated from the same
    /// application UDP flow. TCP activities and legacy UDP records leave this
    /// value nil.
    public let parentFlowIdentifier: UUID?
    /// The provider that observed this flow. Legacy records leave this nil and
    /// are interpreted as ordinary App Routing by consumers.
    public let captureOrigin: AppRoutingActivityCaptureOrigin?
    public var sequence: UInt64
    public let configurationRevision: UInt64
    public let startedAt: Date
    public var endedAt: Date?
    public let source: AppRoutingActivitySource
    public let destination: AppRoutingActivityDestination
    public let transportProtocol: TransportProtocol
    public let decision: FlowTrafficDecision
    public let configuredAction: CaptureAction
    public var effectiveAction: FlowTrafficDisposition
    public let matchedRuleIdentifier: String?
    public let cause: FlowTrafficDecisionReason
    public var relayState: AppRoutingRelayState
    public var relayError: String?
    public var relayNote: String?
    public var relayLocalPort: UInt16?
    /// `true` means the provider owns the payload relay and the byte counters
    /// are exact at its delivery boundaries. `nil` preserves the legacy and
    /// pass-through meaning where payload visibility is not established.
    public var payloadBytesAreMeasured: Bool?
    public var uploadBytes: UInt64
    public var downloadBytes: UInt64
    /// Datagram counters are populated for owned UDP conversations. They stay
    /// nil for TCP and legacy pass-through records so zero is never confused
    /// with "not observable".
    public var uploadDatagrams: UInt64?
    public var downloadDatagrams: UInt64?
    public var droppedDatagrams: UInt64?
    public var lastPayloadAt: Date?

    public var id: UUID { flowIdentifier }
    /// Normalized origin for presentation and attribution. Activity records
    /// written before capture origins were introduced are App Routing records.
    public var effectiveCaptureOrigin: AppRoutingActivityCaptureOrigin {
        captureOrigin ?? .appRouting
    }
    /// Whether the Network Extension still owns this flow and can continue
    /// reporting its lifecycle and payload counters.
    ///
    /// Ordinary Direct and fail-open decisions are handed back to macOS and
    /// are therefore terminal observations rather than live managed flows.
    public var isLiveManagedFlow: Bool {
        guard endedAt == nil else { return false }
        switch relayState {
        case .pending, .connecting, .ready, .relaying:
            return true
        case .notApplicable, .completed, .failed:
            return false
        }
    }
    /// Structured, privacy-bounded rule-decision evidence carried by the
    /// provider snapshot. Legacy activity records return nil.
    public var ruleEvidence: CaptureRuleDecisionEvidence? { decision.ruleEvidence }

    public init(
        flowIdentifier: UUID = UUID(),
        parentFlowIdentifier: UUID? = nil,
        captureOrigin: AppRoutingActivityCaptureOrigin? = .appRouting,
        sequence: UInt64 = 0,
        configurationRevision: UInt64,
        startedAt: Date,
        endedAt: Date? = nil,
        source: AppRoutingActivitySource,
        destination: AppRoutingActivityDestination,
        transportProtocol: TransportProtocol,
        decision: FlowTrafficDecision,
        configuredAction: CaptureAction,
        effectiveAction: FlowTrafficDisposition,
        relayState: AppRoutingRelayState,
        relayError: String? = nil,
        relayNote: String? = nil,
        relayLocalPort: UInt16? = nil,
        payloadBytesAreMeasured: Bool? = nil,
        uploadBytes: UInt64 = 0,
        downloadBytes: UInt64 = 0,
        uploadDatagrams: UInt64? = nil,
        downloadDatagrams: UInt64? = nil,
        droppedDatagrams: UInt64? = nil,
        lastPayloadAt: Date? = nil
    ) {
        self.flowIdentifier = flowIdentifier
        self.parentFlowIdentifier = parentFlowIdentifier
        self.captureOrigin = captureOrigin
        self.sequence = sequence
        self.configurationRevision = configurationRevision
        self.startedAt = startedAt
        self.endedAt = endedAt
        self.source = source
        self.destination = destination
        self.transportProtocol = transportProtocol
        self.decision = decision
        self.configuredAction = configuredAction
        self.effectiveAction = effectiveAction
        matchedRuleIdentifier = Self.matchedRuleIdentifier(in: decision.reason)
        cause = decision.reason
        self.relayState = relayState
        self.relayError = relayError
        self.relayNote = relayNote
        self.relayLocalPort = relayLocalPort
        self.payloadBytesAreMeasured = payloadBytesAreMeasured
        self.uploadBytes = uploadBytes
        self.downloadBytes = downloadBytes
        self.uploadDatagrams = uploadDatagrams
        self.downloadDatagrams = downloadDatagrams
        self.droppedDatagrams = droppedDatagrams
        self.lastPayloadAt = lastPayloadAt
    }

    private static func matchedRuleIdentifier(
        in reason: FlowTrafficDecisionReason
    ) -> String? {
        switch reason {
        case let .rule(cause):
            return matchedRuleIdentifier(in: cause)
        case let .mihomoUnavailable(rule, _):
            return matchedRuleIdentifier(in: rule)
        case .captureDisabled, .configurationUnavailable, .contextUnavailable:
            return nil
        }
    }

    private static func matchedRuleIdentifier(in cause: RuleDecisionCause) -> String? {
        guard case let .matchedRule(identifier) = cause else { return nil }
        return identifier
    }
}

/// A cursor page from an activity ring. Activities are ordered by their most
/// recent sequence. Repeated upserts for the same flow are coalesced to its
/// latest state.
public struct AppRoutingActivityBatch: Codable, Hashable, Sendable {
    public let activities: [AppRoutingActivity]
    public let nextCursor: UInt64
    public let droppedBeforeSequence: UInt64?
    public let hasMore: Bool

    public init(
        activities: [AppRoutingActivity],
        nextCursor: UInt64,
        droppedBeforeSequence: UInt64?,
        hasMore: Bool
    ) {
        self.activities = activities
        self.nextCursor = nextCursor
        self.droppedBeforeSequence = droppedBeforeSequence
        self.hasMore = hasMore
    }
}

/// A lock-protected, active-flow-aware, last-update-ordered activity store.
///
/// Active and terminal flows have separate O(1) indexes. Terminal history is
/// evicted before an active flow, so a long-lived relay cannot disappear merely
/// because many short flows completed after it. `capacity` remains the normal
/// total record bound: history is trimmed to leave room for active flows. When
/// concurrent active flows alone exceed it, they are retained until they become
/// terminal, at which point the history bound is restored.
///
/// Upserting an existing flow assigns a new sequence and coalesces its previous
/// value. Cursor consumers receive only records changed after their cursor.
/// When history eviction creates a gap, `droppedBeforeSequence` tells the
/// consumer that a resynchronization may be required.
public final class BoundedAppRoutingActivityRing: @unchecked Sendable {
    public let capacity: Int

    private let lock = NSLock()
    private var active: [UUID: AppRoutingActivity] = [:]
    private var history: [UUID: HistoryRecord] = [:]
    private var oldestHistoryIdentifier: UUID?
    private var newestHistoryIdentifier: UUID?
    private var nextSequence: UInt64 = 1
    private var droppedBefore: UInt64?

    private struct HistoryRecord {
        var activity: AppRoutingActivity
        var previous: UUID?
        var next: UUID?
    }

    public init(capacity: Int) {
        self.capacity = max(0, capacity)
        active.reserveCapacity(self.capacity)
        history.reserveCapacity(self.capacity)
    }

    public var count: Int {
        withLock { active.count + history.count }
    }

    public var activeCount: Int {
        withLock { active.count }
    }

    public var historyCount: Int {
        withLock { history.count }
    }

    public var latestSequence: UInt64 {
        withLock { nextSequence - 1 }
    }

    public var droppedBeforeSequence: UInt64? {
        withLock { droppedBefore }
    }

    /// Inserts a flow or replaces its previous state and returns the stored
    /// value with the ring-assigned sequence.
    @discardableResult
    public func upsert(_ activity: AppRoutingActivity) -> AppRoutingActivity {
        withLock {
            precondition(nextSequence < UInt64.max, "App Routing activity sequence exhausted")

            var stored = activity
            stored.sequence = nextSequence
            nextSequence += 1

            guard capacity > 0 else {
                markDropped(stored.sequence)
                return stored
            }

            let identifier = stored.flowIdentifier
            active.removeValue(forKey: identifier)
            removeHistory(identifier, markAsDropped: false)

            if stored.isLiveManagedFlow {
                active[identifier] = stored
            } else {
                appendHistory(stored)
            }
            trimHistoryToCapacity()
            return stored
        }
    }

    public func activity(for flowIdentifier: UUID) -> AppRoutingActivity? {
        withLock {
            active[flowIdentifier] ?? history[flowIdentifier]?.activity
        }
    }

    /// Returns records whose most recent update is newer than `cursor`.
    /// Negative limits are treated as zero; limits above capacity are harmless.
    public func batch(after cursor: UInt64 = 0, limit: Int) -> AppRoutingActivityBatch {
        let snapshot: (
            candidates: [AppRoutingActivity]?,
            boundedLimit: Int,
            droppedBefore: UInt64?
        ) = withLock {
            let boundedLimit = min(max(0, limit), capacity)
            guard cursor < nextSequence - 1 else {
                return (nil, boundedLimit, droppedBefore)
            }
            var candidates = active.values.filter { $0.sequence > cursor }
            candidates.append(contentsOf: history.values.lazy
                .map(\.activity)
                .filter { $0.sequence > cursor })
            return (candidates, boundedLimit, droppedBefore)
        }
        guard var candidates = snapshot.candidates else {
            return AppRoutingActivityBatch(
                activities: [],
                nextCursor: cursor,
                droppedBeforeSequence: snapshot.droppedBefore,
                hasMore: false
            )
        }
        // Sorting can dominate this operation at the 2,000-record bound. Do it
        // after copying the value snapshot so flow admission and counter upserts
        // do not wait behind O(n log n) work on the ring's only lock.
        candidates.sort { $0.sequence < $1.sequence }
        let selected = Array(candidates.prefix(snapshot.boundedLimit))
        return AppRoutingActivityBatch(
            activities: selected,
            nextCursor: selected.last?.sequence ?? cursor,
            droppedBeforeSequence: snapshot.droppedBefore,
            hasMore: candidates.count > selected.count
        )
    }

    /// Clears retained records without reusing sequences. Existing cursors can
    /// therefore detect that records were discarded.
    public func removeAll() {
        withLock {
            if !active.isEmpty || !history.isEmpty {
                markDropped(nextSequence - 1)
            }
            active.removeAll(keepingCapacity: true)
            removeAllHistory()
        }
    }

    /// Clears completed/direct history while preserving live relay records.
    /// This is the provider-facing clear operation: active flows remain
    /// observable and continue accepting counter/final-state updates.
    public func removeHistory() {
        withLock {
            if let newestHistoryIdentifier,
               let newest = history[newestHistoryIdentifier]?.activity {
                markDropped(newest.sequence)
            }
            removeAllHistory()
        }
    }

    private func appendHistory(_ activity: AppRoutingActivity) {
        let identifier = activity.flowIdentifier
        history[identifier] = HistoryRecord(
            activity: activity,
            previous: newestHistoryIdentifier,
            next: nil
        )
        if let newestHistoryIdentifier {
            history[newestHistoryIdentifier]?.next = identifier
        } else {
            oldestHistoryIdentifier = identifier
        }
        newestHistoryIdentifier = identifier
    }

    private func removeHistory(_ identifier: UUID, markAsDropped: Bool) {
        guard let removed = history.removeValue(forKey: identifier) else { return }
        if let previous = removed.previous {
            history[previous]?.next = removed.next
        } else {
            oldestHistoryIdentifier = removed.next
        }
        if let next = removed.next {
            history[next]?.previous = removed.previous
        } else {
            newestHistoryIdentifier = removed.previous
        }
        if markAsDropped {
            markDropped(removed.activity.sequence)
        }
    }

    private func trimHistoryToCapacity() {
        while active.count + history.count > capacity,
              let oldestHistoryIdentifier {
            removeHistory(oldestHistoryIdentifier, markAsDropped: true)
        }
    }

    private func removeAllHistory() {
        history.removeAll(keepingCapacity: true)
        oldestHistoryIdentifier = nil
        newestHistoryIdentifier = nil
    }

    private func markDropped(_ sequence: UInt64) {
        droppedBefore = max(droppedBefore ?? 0, sequence)
    }

    private func withLock<T>(_ operation: () throws -> T) rethrows -> T {
        lock.lock()
        defer { lock.unlock() }
        return try operation()
    }
}

/// Pure state machine used by Network Extension relays to coalesce high-rate
/// byte-counter activity without delaying lifecycle or terminal states.
public struct AppRoutingRelayReportLimiter: Sendable {
    public enum Decision: Equatable, Sendable {
        case emit
        case schedule(afterNanoseconds: UInt64)
        case suppress
    }

    private var lastRelayingReportAt: UInt64?
    private var scheduledDeadline: UInt64?

    private static let minimumIntervalNanoseconds: UInt64 = 250_000_000

    public init() {}

    /// Lifecycle states always emit immediately. Relaying updates emit on the
    /// first sample, then at most once per interval.
    public mutating func decision(
        for state: AppRoutingRelayState,
        nowNanoseconds: UInt64
    ) -> Decision {
        guard state == .relaying else {
            scheduledDeadline = nil
            return .emit
        }

        guard let lastRelayingReportAt else {
            recordRelayingReport(at: nowNanoseconds)
            return .emit
        }

        let nextReport = lastRelayingReportAt.addingReportingOverflow(
            Self.minimumIntervalNanoseconds
        )
        let deadline = nextReport.overflow ? UInt64.max : nextReport.partialValue
        if nowNanoseconds >= deadline {
            recordRelayingReport(at: nowNanoseconds)
            return .emit
        }

        if scheduledDeadline == nil {
            let remaining = deadline - nowNanoseconds
            scheduledDeadline = deadline
            return .schedule(afterNanoseconds: remaining)
        }
        return .suppress
    }

    /// Flushes a previously scheduled trailing report.
    public mutating func shouldEmitScheduledReport(nowNanoseconds: UInt64) -> Bool {
        guard let scheduledDeadline,
              nowNanoseconds >= scheduledDeadline
        else { return false }
        recordRelayingReport(at: nowNanoseconds)
        return true
    }

    private mutating func recordRelayingReport(at nowNanoseconds: UInt64) {
        lastRelayingReportAt = nowNanoseconds
        scheduledDeadline = nil
    }
}
