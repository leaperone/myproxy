import Foundation
import MyproxyNetworkShared

/// Aggregates DNS relay lifecycle and delivery-boundary bytes into a bounded
/// snapshot published through the process-local provider registry.
final class DNSProxyRuntimeReporter: @unchecked Sendable {
    private struct FlowState {
        let transportProtocol: TransportProtocol
        var uploadBytes: UInt64 = 0
        var downloadBytes: UInt64 = 0
        var terminal = false
    }

    private let lock = NSLock()
    private let registry: DNSProxyRuntimeRegistry
    private let heartbeatQueue = DispatchQueue(
        label: "local.harry.myproxy.dns-runtime-heartbeat"
    )
    private var heartbeat: DispatchSourceTimer?
    private var status: DNSProxyRuntimeStatus
    private var flows: [UUID: FlowState] = [:]
    private var terminalFlowIdentifiers: Set<UUID> = []
    private var terminalFlowOrder: [UUID] = []
    private var terminalFlowEvictionIndex = 0
    private var stopped = false

    private static let maximumTerminalFlowIdentifiers = 4_096

    init(
        revision: UInt64,
        activationIdentifier: UUID,
        now: Date = Date(),
        registry: DNSProxyRuntimeRegistry = .shared
    ) throws {
        self.registry = registry
        status = DNSProxyRuntimeStatus(
            revision: revision,
            activationIdentifier: activationIdentifier,
            phase: .starting,
            backendReady: false,
            startedAt: now
        )
        try registry.publish(status)
    }

    func resumeHeartbeat() {
        lock.lock()
        guard heartbeat == nil, !stopped else {
            lock.unlock()
            return
        }
        status.recordHeartbeat()
        try? registry.publish(status)
        let timer = DispatchSource.makeTimerSource(queue: heartbeatQueue)
        timer.schedule(
            deadline: .now() + .seconds(3),
            repeating: .seconds(3),
            leeway: .seconds(1)
        )
        timer.setEventHandler { [weak self] in self?.recordHeartbeat() }
        timer.resume()
        heartbeat = timer
        lock.unlock()
    }

    func pauseHeartbeat() {
        lock.lock()
        let timer = heartbeat
        heartbeat = nil
        lock.unlock()
        timer?.setEventHandler {}
        timer?.cancel()
    }

    func markRunning() throws {
        try mutateAndPublish { value, now in
            guard !self.stopped else { return }
            value.phase = .running
            value.backendReady = true
            value.lastBackendAssociationAt = now
            value.updatedAt = now
        }
    }

    func beginFlow(_ identifier: UUID, transportProtocol: TransportProtocol) {
        publishBestEffort { value, now in
            guard !self.stopped else { return }
            guard self.flows[identifier] == nil,
                  !self.terminalFlowIdentifiers.contains(identifier) else { return }
            self.flows[identifier] = FlowState(transportProtocol: transportProtocol)
            value.totalFlows = Self.increment(value.totalFlows)
            switch transportProtocol {
            case .tcp: value.activeTCPFlows = Self.increment(value.activeTCPFlows)
            case .udp: value.activeUDPFlows = Self.increment(value.activeUDPFlows)
            }
            value.updatedAt = now
        }
    }

    func observe(
        _ snapshot: AppRoutingRelaySnapshot,
        flowIdentifier: UUID,
        transportProtocol: TransportProtocol
    ) {
        publishBestEffort { value, now in
            guard !self.stopped else { return }
            guard !self.terminalFlowIdentifiers.contains(flowIdentifier) else { return }
            if self.flows[flowIdentifier] == nil {
                self.flows[flowIdentifier] = FlowState(
                    transportProtocol: transportProtocol
                )
                value.totalFlows = Self.increment(value.totalFlows)
                switch transportProtocol {
                case .tcp: value.activeTCPFlows = Self.increment(value.activeTCPFlows)
                case .udp: value.activeUDPFlows = Self.increment(value.activeUDPFlows)
                }
            }
            guard var flow = self.flows[flowIdentifier], !flow.terminal else { return }
            let uploadDelta = Self.delta(
                new: snapshot.uploadBytes,
                old: flow.uploadBytes
            )
            let downloadDelta = Self.delta(
                new: snapshot.downloadBytes,
                old: flow.downloadBytes
            )
            value.uploadBytes = Self.add(value.uploadBytes, uploadDelta)
            value.downloadBytes = Self.add(value.downloadBytes, downloadDelta)
            if uploadDelta > 0 {
                value.lastQueryForwardedAt = now
            }
            if downloadDelta > 0 {
                value.lastResponseDeliveredAt = now
            }
            flow.uploadBytes = max(flow.uploadBytes, snapshot.uploadBytes)
            flow.downloadBytes = max(flow.downloadBytes, snapshot.downloadBytes)

            let becameTerminal: Bool
            switch snapshot.state {
            case .ready, .relaying:
                becameTerminal = false
            case .completed:
                flow.terminal = true
                Self.decrementActive(&value, transportProtocol: flow.transportProtocol)
                value.completedFlows = Self.increment(value.completedFlows)
                becameTerminal = true
            case .failed:
                flow.terminal = true
                Self.decrementActive(&value, transportProtocol: flow.transportProtocol)
                value.failedFlows = Self.increment(value.failedFlows)
                value.lastFailureAt = now
                value.failureCategory = transportProtocol == .tcp
                    ? .tcpRelayFailed
                    : .udpRelayFailed
                becameTerminal = true
            case .notApplicable, .pending, .connecting:
                becameTerminal = false
            }
            if becameTerminal {
                self.flows.removeValue(forKey: flowIdentifier)
                self.rememberTerminalFlow(flowIdentifier)
            } else {
                self.flows[flowIdentifier] = flow
            }
            value.updatedAt = now
        }
    }

    func markStartupFailed(_ category: DNSProxyFailureCategory) {
        lock.lock()
        guard !stopped else {
            lock.unlock()
            return
        }
        stopped = true
        let timer = heartbeat
        heartbeat = nil
        let now = Date()
        status.phase = .failed
        status.backendReady = false
        status.activeTCPFlows = 0
        status.activeUDPFlows = 0
        status.lastFailureAt = now
        status.failureCategory = category
        status.updatedAt = now
        flows.removeAll(keepingCapacity: false)
        terminalFlowIdentifiers.removeAll(keepingCapacity: false)
        terminalFlowOrder.removeAll(keepingCapacity: false)
        terminalFlowEvictionIndex = 0
        let failedStatus = status
        lock.unlock()
        timer?.setEventHandler {}
        timer?.cancel()
        try? registry.publish(failedStatus)
    }

    func markBackendUnavailable(_ category: DNSProxyFailureCategory) {
        publishBestEffort { value, now in
            guard !self.stopped else { return }
            value.phase = .running
            value.backendReady = false
            value.lastFailureAt = now
            value.failureCategory = category
            value.updatedAt = now
        }
    }

    /// Records the latest per-flow failure without changing the independently
    /// probed backend health. One malformed or failed DNS flow must not make
    /// the entire provider appear offline.
    func recordFlowFailure(_ category: DNSProxyFailureCategory) {
        publishBestEffort { value, now in
            guard !self.stopped else { return }
            value.lastFailureAt = now
            value.failureCategory = category
            value.updatedAt = now
        }
    }

    func stop(category: DNSProxyFailureCategory? = nil) {
        lock.lock()
        stopped = true
        let timer = heartbeat
        heartbeat = nil
        lock.unlock()
        timer?.setEventHandler {}
        timer?.cancel()

        publishBestEffort { value, now in
            let active = value.activeFlows
            value.failedFlows = Self.add(value.failedFlows, active)
            value.activeTCPFlows = 0
            value.activeUDPFlows = 0
            value.backendReady = false
            value.phase = .stopped
            if let category, active > 0 {
                value.lastFailureAt = now
                value.failureCategory = category
            }
            value.updatedAt = now
            self.flows.removeAll(keepingCapacity: false)
            self.terminalFlowIdentifiers.removeAll(keepingCapacity: false)
            self.terminalFlowOrder.removeAll(keepingCapacity: false)
            self.terminalFlowEvictionIndex = 0
        }
    }

    private func recordHeartbeat() {
        publishBestEffort { value, now in
            guard !self.stopped else { return }
            value.updatedAt = now
        }
    }

    private func rememberTerminalFlow(_ identifier: UUID) {
        guard terminalFlowIdentifiers.insert(identifier).inserted else { return }
        if terminalFlowOrder.count < Self.maximumTerminalFlowIdentifiers {
            terminalFlowOrder.append(identifier)
            return
        }
        let expired = terminalFlowOrder[terminalFlowEvictionIndex]
        terminalFlowIdentifiers.remove(expired)
        terminalFlowOrder[terminalFlowEvictionIndex] = identifier
        terminalFlowEvictionIndex = (terminalFlowEvictionIndex + 1)
            % Self.maximumTerminalFlowIdentifiers
    }

    private func publishBestEffort(
        _ mutation: (inout DNSProxyRuntimeStatus, Date) -> Void
    ) {
        try? mutateAndPublish(mutation)
    }

    private func mutateAndPublish(
        _ mutation: (inout DNSProxyRuntimeStatus, Date) -> Void
    ) throws {
        lock.lock()
        defer { lock.unlock() }
        mutation(&status, Date())
        try registry.publish(status)
    }

    private static func decrementActive(
        _ status: inout DNSProxyRuntimeStatus,
        transportProtocol: TransportProtocol
    ) {
        switch transportProtocol {
        case .tcp:
            status.activeTCPFlows = status.activeTCPFlows > 0
                ? status.activeTCPFlows - 1
                : 0
        case .udp:
            status.activeUDPFlows = status.activeUDPFlows > 0
                ? status.activeUDPFlows - 1
                : 0
        }
    }

    private static func increment(_ value: UInt64) -> UInt64 {
        value == .max ? .max : value + 1
    }

    private static func add(_ lhs: UInt64, _ rhs: UInt64) -> UInt64 {
        let (value, overflow) = lhs.addingReportingOverflow(rhs)
        return overflow ? .max : value
    }

    private static func delta(new: UInt64, old: UInt64) -> UInt64 {
        new >= old ? new - old : 0
    }
}
