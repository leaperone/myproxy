import Foundation
import MyproxyNetworkShared
@preconcurrency import Network
@preconcurrency import NetworkExtension

enum UDPFlowSessionError: Error, LocalizedError, Sendable {
    case cancelled
    case flowOpenFailed(String)
    case flowReadFailed(String)
    case flowWriteFailed(String)
    case datagramTooLarge(limit: Int, actual: Int)
    case outboundQueueFull(maximumDatagrams: Int, maximumBytes: Int)
    case responseQueueFull(maximumDatagrams: Int, maximumBytes: Int)
    case tooManyConversations(limit: Int)
    case globalConversationLimit(limit: Int)
    case unsupportedDestination
    case rejected(String)
    case directConnectionFailed(String)
    case mihomoSetupFailed(String)
    case mihomoRelayFailed(String)
    case incompleteDatagram

    var errorDescription: String? {
        switch self {
        case .cancelled:
            "The UDP flow stopped because App Routing stopped, slept, or changed generation."
        case let .flowOpenFailed(message):
            "The intercepted UDP flow could not be opened: \(message)"
        case let .flowReadFailed(message):
            "Reading the intercepted UDP flow failed: \(message)"
        case let .flowWriteFailed(message):
            "Writing the intercepted UDP flow failed: \(message)"
        case let .datagramTooLarge(limit, actual):
            "The intercepted UDP datagram is \(actual) bytes; limit is \(limit)."
        case let .outboundQueueFull(datagrams, bytes):
            "The UDP outbound queue reached its safety limit of \(datagrams) datagrams or \(bytes) bytes."
        case let .responseQueueFull(datagrams, bytes):
            "The UDP response queue reached its safety limit of \(datagrams) datagrams or \(bytes) bytes."
        case let .tooManyConversations(limit):
            "This UDP socket exceeded its safety limit of \(limit) independently routed destinations."
        case let .globalConversationLimit(limit):
            "App Routing reached its safety limit of \(limit) active UDP destinations."
        case .unsupportedDestination:
            "The UDP destination could not be represented safely."
        case let .rejected(message):
            message
        case let .directConnectionFailed(message):
            "The direct UDP connection failed: \(message)"
        case let .mihomoSetupFailed(message):
            "The local Mihomo SOCKS5 UDP association failed before payload forwarding: \(message)"
        case let .mihomoRelayFailed(message):
            "The Mihomo UDP relay failed: \(message)"
        case .incompleteDatagram:
            "An upstream UDP connection returned an incomplete datagram."
        }
    }
}

private enum UDPConversationFailureStage: Sendable {
    case setup
    case relaying
}

private protocol UDPConversation: AnyObject, Sendable {
    var id: UUID { get }
    var activityIdentifier: UUID { get }
    var endpoint: SOCKS5Endpoint { get }

    func start()
    func send(
        _ payload: Data,
        completion: @escaping @Sendable (Error?) -> Void
    )
    func responseDelivered(bytes: Int)
    func resumeReceiving()
    func stop(error: Error?, reportTerminal: Bool)
}

/// One owned NE UDP flow. Rule evaluation and upstream state are scoped to a
/// per-destination conversation so an unconnected UDP socket cannot reuse its
/// first packet's IP/port decision for later destinations.
final class UDPFlowSession: @unchecked Sendable {
    let id: UUID

    private enum Limits {
        static let maximumConversations = 64
        static let maximumQueuedOutboundDatagrams = 512
        static let maximumQueuedOutboundBytes = 4 * 1_024 * 1_024
        static let maximumQueuedResponseDatagrams = 128
        static let maximumQueuedResponseBytes = 4 * 1_024 * 1_024
    }

    private struct ConversationKey: Hashable, Sendable {
        let destination: SOCKS5Endpoint
        let revision: UInt64
    }

    private final class ConversationRecord: @unchecked Sendable {
        let key: ConversationKey
        let plan: UDPFlowInterceptionPlan
        var conversation: (any UDPConversation)?
        var pendingPayloads: ArraySlice<Data?> = []
        var ready = false
        var sendInFlight = false
        var mihomoPayloadForwarded = false
        var admissionHeld = false

        init(key: ConversationKey, plan: UDPFlowInterceptionPlan) {
            self.key = key
            self.plan = plan
        }
    }

    private struct PendingResponse: Sendable {
        let conversationIdentifier: UUID
        let datagram: UDPFlowDatagram
    }

    private let flow: NEAppProxyUDPFlow
    private let initialPlan: UDPFlowInterceptionPlan
    private let queue: DispatchQueue
    private let planner: @Sendable (SOCKS5Endpoint) -> UDPFlowInterceptionPlan
    private let revisionProvider: @Sendable () -> UInt64
    private let activitySink: @Sendable (AppRoutingActivity) -> Void
    private let observerFactory:
        @Sendable (UUID) -> @Sendable (AppRoutingRelaySnapshot) -> Void
    private let acquireConversationAdmission: @Sendable () -> Bool
    private let releaseConversationAdmission: @Sendable () -> Void
    private let completion: @Sendable (UUID) -> Void

    private var records: [ConversationKey: ConversationRecord] = [:]
    private var recordKeyByConversationID: [UUID: ConversationKey] = [:]
    private var initialPlanAvailable = true
    private var outboundBudget = UDPRelayQueueBudget(
        maximumDatagrams: Limits.maximumQueuedOutboundDatagrams,
        maximumBytes: Limits.maximumQueuedOutboundBytes
    )
    private var responseBudget = UDPRelayQueueBudget(
        maximumDatagrams: Limits.maximumQueuedResponseDatagrams,
        maximumBytes: Limits.maximumQueuedResponseBytes
    )
    private var pendingResponses: ArraySlice<PendingResponse?> = []
    private var readInFlight = false
    private var writeInFlight = false
    private var opened = false
    private var finished = false

    init(
        id: UUID,
        flow: NEAppProxyUDPFlow,
        initialPlan: UDPFlowInterceptionPlan,
        planner: @escaping @Sendable (SOCKS5Endpoint) -> UDPFlowInterceptionPlan,
        revisionProvider: @escaping @Sendable () -> UInt64,
        activitySink: @escaping @Sendable (AppRoutingActivity) -> Void,
        observerFactory:
            @escaping @Sendable (UUID) -> @Sendable (AppRoutingRelaySnapshot) -> Void,
        acquireConversationAdmission: @escaping @Sendable () -> Bool,
        releaseConversationAdmission: @escaping @Sendable () -> Void,
        completion: @escaping @Sendable (UUID) -> Void
    ) {
        self.id = id
        self.flow = flow
        self.initialPlan = initialPlan
        self.planner = planner
        self.revisionProvider = revisionProvider
        self.activitySink = activitySink
        self.observerFactory = observerFactory
        self.acquireConversationAdmission = acquireConversationAdmission
        self.releaseConversationAdmission = releaseConversationAdmission
        self.completion = completion
        queue = DispatchQueue(label: "local.harry.myproxy.udp-session.\(id.uuidString)")
    }

    func start() {
        queue.async { [self] in
            guard !finished else { return }
            publishInitialActivity()
            AppProxyFlowCompatibility.open(flow) { [weak self] error in
                guard let self else { return }
                self.queue.async {
                    if let error {
                        self.finish(error: UDPFlowSessionError.flowOpenFailed(
                            error.localizedDescription
                        ))
                        return
                    }
                    self.opened = true
                    self.readFromFlowIfPossible()
                }
            }
        }
    }

    func cancel() {
        queue.async { [self] in
            finish(error: UDPFlowSessionError.cancelled)
        }
    }

    private func publishInitialActivity() {
        var activity = initialPlan.activity
        activity.endedAt = nil
        activity.relayState = .connecting
        activity.payloadBytesAreMeasured = true
        activity.uploadDatagrams = 0
        activity.downloadDatagrams = 0
        activity.droppedDatagrams = 0
        activitySink(activity)
    }

    private func readFromFlowIfPossible() {
        guard !finished, opened, !readInFlight else { return }
        guard outboundBudget.datagramCount
                < Limits.maximumQueuedOutboundDatagrams * 3 / 4,
              outboundBudget.byteCount
                < Limits.maximumQueuedOutboundBytes * 3 / 4 else { return }
        readInFlight = true
        UDPAppProxyFlowCompatibility.read(from: flow) { [weak self] datagrams, error in
            guard let self else { return }
            self.queue.async {
                self.readInFlight = false
                if let error {
                    self.finish(error: UDPFlowSessionError.flowReadFailed(
                        error.localizedDescription
                    ))
                    return
                }
                guard let datagrams, !datagrams.isEmpty else {
                    self.finish(error: nil)
                    return
                }
                do {
                    for datagram in datagrams {
                        try self.enqueue(datagram)
                    }
                    self.readFromFlowIfPossible()
                } catch {
                    self.finish(error: error)
                }
            }
        }
    }

    private func enqueue(_ datagram: UDPFlowDatagram) throws {
        guard datagram.payload.count <= SOCKS5Limits.maximumUDPDatagramBytes else {
            throw UDPFlowSessionError.datagramTooLarge(
                limit: SOCKS5Limits.maximumUDPDatagramBytes,
                actual: datagram.payload.count
            )
        }
        guard outboundBudget.reserve(bytes: datagram.payload.count) else {
            throw UDPFlowSessionError.outboundQueueFull(
                maximumDatagrams: Limits.maximumQueuedOutboundDatagrams,
                maximumBytes: Limits.maximumQueuedOutboundBytes
            )
        }

        do {
            let record = try conversationRecord(for: datagram.endpoint)
            record.pendingPayloads.append(datagram.payload)
            drain(record)
        } catch {
            outboundBudget.release(bytes: datagram.payload.count)
            throw error
        }
    }

    private func conversationRecord(
        for destination: SOCKS5Endpoint
    ) throws -> ConversationRecord {
        let revision = revisionProvider()
        let key = ConversationKey(destination: destination, revision: revision)
        if let existing = records[key] { return existing }
        guard records.count < Limits.maximumConversations else {
            throw UDPFlowSessionError.tooManyConversations(
                limit: Limits.maximumConversations
            )
        }
        guard acquireConversationAdmission() else {
            throw UDPFlowSessionError.globalConversationLimit(
                limit: UDPFlowSessionRegistry.maximumActiveConversations
            )
        }

        let plan: UDPFlowInterceptionPlan
        if initialPlanAvailable,
           initialPlan.configurationRevision == revision,
           initialPlan.initialDestination == destination {
            initialPlanAvailable = false
            plan = initialPlan
        } else {
            plan = planner(destination)
        }

        let record = ConversationRecord(key: key, plan: plan)
        record.admissionHeld = true
        records[key] = record
        do {
            try configure(record)
            return record
        } catch {
            remove(record)
            throw error
        }
    }

    private func configure(_ record: ConversationRecord) throws {
        var activity = record.plan.activity
        activity.endedAt = nil
        activity.relayState = .connecting
        activity.payloadBytesAreMeasured = true
        activity.uploadDatagrams = 0
        activity.downloadDatagrams = 0
        activity.droppedDatagrams = 0
        activitySink(activity)

        switch record.plan.decision.disposition {
        case .direct:
            configureDirect(record, note: directFallbackNote(for: record.plan))
        case .failOpen:
            configureDirect(
                record,
                note: "The UDP flow was already owned when fail-open was requested; myproxy relayed this destination directly and measured it instead of silently losing the packet."
            )
        case .reject:
            publishRejection(record, reason: "A matching App Routing rule rejected this UDP destination.")
            throw UDPFlowSessionError.rejected(
                "UDP destination \(Self.description(record.key.destination)) was rejected by App Routing."
            )
        case .mihomo:
            guard let proxy = record.plan.proxy else {
                switch record.plan.unavailableFallback {
                case .direct:
                    configureDirect(
                        record,
                        note: "The requested Mihomo UDP route had no route-specific listener, so this destination used the rule's Direct fallback."
                    )
                case .reject:
                    publishRejection(
                        record,
                        reason: "The requested Mihomo UDP route was unavailable and this rule rejects fallback."
                    )
                    throw UDPFlowSessionError.rejected(
                        "The requested Mihomo UDP route was unavailable and fallback is Reject."
                    )
                }
                return
            }
            guard let mihomoDestination = record.plan.mihomoDestination else {
                throw UDPFlowSessionError.unsupportedDestination
            }
            let conversation = MihomoUDPConversation(
                queue: queue,
                activityIdentifier: activity.flowIdentifier,
                endpoint: mihomoDestination,
                proxy: proxy,
                observer: observerFactory(activity.flowIdentifier),
                ready: conversationReady,
                response: conversationResponse,
                failure: conversationFailed
            )
            record.conversation = conversation
            recordKeyByConversationID[conversation.id] = record.key
            conversation.start()
        }
    }

    private func configureDirect(
        _ record: ConversationRecord,
        note: String?
    ) {
        let conversation = DirectUDPConversation(
            queue: queue,
            flow: flow,
            activityIdentifier: record.plan.activity.flowIdentifier,
            endpoint: record.key.destination,
            note: note,
            observer: observerFactory(record.plan.activity.flowIdentifier),
            ready: conversationReady,
            response: conversationResponse,
            failure: conversationFailed
        )
        record.conversation = conversation
        recordKeyByConversationID[conversation.id] = record.key
        conversation.start()
    }

    private var conversationReady: @Sendable (UUID) -> Void {
        { [weak self] identifier in
            guard let self else { return }
            self.queue.async {
                guard let record = self.record(forConversation: identifier) else { return }
                record.ready = true
                self.drain(record)
            }
        }
    }

    private var conversationResponse: @Sendable (UUID, UDPFlowDatagram) -> Void {
        { [weak self] identifier, datagram in
            guard let self else { return }
            self.queue.async {
                guard let record = self.record(forConversation: identifier) else { return }
                self.enqueueResponse(
                    PendingResponse(
                        conversationIdentifier: identifier,
                        // A hostname may be used upstream so Mihomo can apply
                        // domain rules, but the app must still observe replies
                        // from the exact endpoint it originally addressed.
                        datagram: UDPFlowDatagram(
                            payload: datagram.payload,
                            endpoint: record.key.destination
                        )
                    )
                )
            }
        }
    }

    private var conversationFailed:
        @Sendable (UUID, Error, UDPConversationFailureStage) -> Void {
        { [weak self] identifier, error, stage in
            guard let self else { return }
            self.queue.async {
                self.handleConversationFailure(
                    identifier: identifier,
                    error: error,
                    stage: stage
                )
            }
        }
    }

    private func drain(_ record: ConversationRecord) {
        guard !finished,
              record.ready,
              !record.sendInFlight,
              let conversation = record.conversation,
              let payload = record.pendingPayloads.first ?? nil else { return }
        record.sendInFlight = true
        conversation.send(payload) { [weak self] error in
            guard let self else { return }
            self.queue.async {
                guard !self.finished else { return }
                record.sendInFlight = false
                if let error {
                    self.finish(error: error)
                    return
                }
                if record.conversation is MihomoUDPConversation {
                    record.mihomoPayloadForwarded = true
                }
                record.pendingPayloads[record.pendingPayloads.startIndex] = nil
                record.pendingPayloads.removeFirst()
                self.outboundBudget.release(bytes: payload.count)
                self.drain(record)
                self.readFromFlowIfPossible()
            }
        }
    }

    private func enqueueResponse(_ response: PendingResponse) {
        guard !finished else { return }
        guard responseBudget.reserve(bytes: response.datagram.payload.count) else {
            finish(error: UDPFlowSessionError.responseQueueFull(
                maximumDatagrams: Limits.maximumQueuedResponseDatagrams,
                maximumBytes: Limits.maximumQueuedResponseBytes
            ))
            return
        }
        pendingResponses.append(response)
        writeNextResponseIfNeeded()
    }

    private func writeNextResponseIfNeeded() {
        guard !finished,
              !writeInFlight,
              let response = pendingResponses.first ?? nil else { return }
        writeInFlight = true
        UDPAppProxyFlowCompatibility.write(response.datagram, to: flow) { [weak self] error in
            guard let self else { return }
            self.queue.async {
                guard !self.finished else { return }
                self.writeInFlight = false
                if let error {
                    self.finish(error: UDPFlowSessionError.flowWriteFailed(
                        error.localizedDescription
                    ))
                    return
                }
                self.pendingResponses[self.pendingResponses.startIndex] = nil
                self.pendingResponses.removeFirst()
                self.responseBudget.release(bytes: response.datagram.payload.count)
                if let record = self.record(forConversation: response.conversationIdentifier),
                   let conversation = record.conversation {
                    conversation.responseDelivered(bytes: response.datagram.payload.count)
                    conversation.resumeReceiving()
                }
                self.writeNextResponseIfNeeded()
            }
        }
    }

    private func handleConversationFailure(
        identifier: UUID,
        error: Error,
        stage: UDPConversationFailureStage
    ) {
        guard let record = record(forConversation: identifier) else { return }
        if stage == .setup,
           record.conversation is MihomoUDPConversation,
           !record.mihomoPayloadForwarded,
           record.plan.unavailableFallback == .direct {
            record.conversation?.stop(error: nil, reportTerminal: false)
            recordKeyByConversationID.removeValue(forKey: identifier)
            record.conversation = nil
            record.ready = false
            record.sendInFlight = false
            configureDirect(
                record,
                note: "Mihomo SOCKS5 UDP setup failed before payload forwarding; this destination used the rule's Direct fallback. \(error.localizedDescription)"
            )
            return
        }
        finish(error: error)
    }

    private func publishRejection(
        _ record: ConversationRecord,
        reason: String
    ) {
        observerFactory(record.plan.activity.flowIdentifier)(
            AppRoutingRelaySnapshot(
                state: .failed,
                uploadBytes: 0,
                downloadBytes: 0,
                error: reason,
                localPort: nil,
                effectiveAction: .reject,
                uploadDatagrams: 0,
                downloadDatagrams: 0,
                droppedDatagrams: 1,
                lastPayloadAt: Date()
            )
        )
    }

    private func record(forConversation identifier: UUID) -> ConversationRecord? {
        guard let key = recordKeyByConversationID[identifier] else { return nil }
        return records[key]
    }

    private func remove(_ record: ConversationRecord) {
        if let conversation = record.conversation {
            recordKeyByConversationID.removeValue(forKey: conversation.id)
            conversation.stop(error: nil, reportTerminal: false)
        }
        records.removeValue(forKey: record.key)
        if record.admissionHeld {
            record.admissionHeld = false
            releaseConversationAdmission()
        }
    }

    private func finish(error: Error?) {
        guard !finished else { return }
        finished = true
        for record in records.values {
            record.conversation?.stop(error: error, reportTerminal: true)
            if record.admissionHeld {
                record.admissionHeld = false
                releaseConversationAdmission()
            }
        }
        records.removeAll(keepingCapacity: false)
        recordKeyByConversationID.removeAll(keepingCapacity: false)
        pendingResponses.removeAll(keepingCapacity: false)
        if let error {
            flow.closeReadWithError(error)
            flow.closeWriteWithError(error)
        } else if opened {
            flow.closeReadWithError(nil)
            flow.closeWriteWithError(nil)
        }
        completion(id)
    }

    private func directFallbackNote(for plan: UDPFlowInterceptionPlan) -> String? {
        guard case .mihomoUnavailable = plan.decision.reason else { return nil }
        return "Mihomo was unavailable when this destination was decided, so the rule's Direct fallback was used."
    }

    private static func description(_ endpoint: SOCKS5Endpoint) -> String {
        let host = endpoint.address.ipAddress?.presentation
            ?? endpoint.address.domain
            ?? "unknown"
        return "\(host):\(endpoint.port)"
    }
}

/// A single destination on an owned UDP flow, connected directly while
/// explicitly bypassing system proxy settings.
private final class DirectUDPConversation: UDPConversation, @unchecked Sendable {
    let id = UUID()
    let activityIdentifier: UUID
    let endpoint: SOCKS5Endpoint

    private let queue: DispatchQueue
    private let flow: NEAppProxyUDPFlow
    private let note: String?
    private let reporter: AppRoutingRelayActivityReporter
    private let readyCallback: @Sendable (UUID) -> Void
    private let responseCallback: @Sendable (UUID, UDPFlowDatagram) -> Void
    private let failureCallback:
        @Sendable (UUID, Error, UDPConversationFailureStage) -> Void
    private var connection: NWConnection?
    private var ready = false
    private var receiveInFlight = false
    private var finished = false
    private var byteLedger = UDPRelayByteLedger()
    private var uploadDatagrams: UInt64 = 0
    private var downloadDatagrams: UInt64 = 0
    private var localPort: UInt16?
    private var lastPayloadAt: Date?

    init(
        queue: DispatchQueue,
        flow: NEAppProxyUDPFlow,
        activityIdentifier: UUID,
        endpoint: SOCKS5Endpoint,
        note: String?,
        observer: @escaping @Sendable (AppRoutingRelaySnapshot) -> Void,
        ready: @escaping @Sendable (UUID) -> Void,
        response: @escaping @Sendable (UUID, UDPFlowDatagram) -> Void,
        failure: @escaping @Sendable (UUID, Error, UDPConversationFailureStage) -> Void
    ) {
        self.queue = queue
        self.flow = flow
        self.activityIdentifier = activityIdentifier
        self.endpoint = endpoint
        self.note = note
        readyCallback = ready
        responseCallback = response
        failureCallback = failure
        reporter = AppRoutingRelayActivityReporter(queue: queue, observer: observer)
    }

    func start() {
        do {
            guard endpoint.port > 0,
                  let port = NWEndpoint.Port(rawValue: endpoint.port),
                  let host = endpoint.address.ipAddress?.presentation
                    ?? endpoint.address.domain else {
                throw UDPFlowSessionError.unsupportedDestination
            }
            report(.connecting)
            let parameters = NWParameters.udp
            parameters.preferNoProxies = true
            if #available(macOS 15.0, *) {
                flow.setMetadata(on: parameters)
            }
            let connection = NWConnection(
                host: NWEndpoint.Host(host),
                port: port,
                using: parameters
            )
            self.connection = connection
            connection.stateUpdateHandler = { [weak self] state in
                guard let self else { return }
                self.handle(state)
            }
            connection.start(queue: queue)
        } catch {
            fail(error, stage: .setup)
        }
    }

    func send(
        _ payload: Data,
        completion: @escaping @Sendable (Error?) -> Void
    ) {
        guard !finished, ready, let connection else {
            completion(UDPFlowSessionError.directConnectionFailed("The connection is not ready."))
            return
        }
        connection.send(
            content: payload,
            contentContext: .defaultMessage,
            isComplete: true,
            completion: .contentProcessed { [weak self] error in
                guard let self else { return }
                if let error {
                    let failure = UDPFlowSessionError.directConnectionFailed(
                        error.localizedDescription
                    )
                    completion(failure)
                    self.fail(failure, stage: .relaying)
                    return
                }
                self.byteLedger.recordUpstreamAccepted(payload.count)
                self.uploadDatagrams = Self.increment(self.uploadDatagrams)
                self.lastPayloadAt = Date()
                self.report(.relaying)
                completion(nil)
            }
        )
    }

    func responseDelivered(bytes: Int) {
        guard !finished else { return }
        byteLedger.recordApplicationDelivered(bytes)
        downloadDatagrams = Self.increment(downloadDatagrams)
        lastPayloadAt = Date()
        report(.relaying)
    }

    func resumeReceiving() {
        receiveNext()
    }

    func stop(error: Error?, reportTerminal: Bool) {
        guard !finished else { return }
        finished = true
        connection?.stateUpdateHandler = nil
        connection?.cancel()
        connection = nil
        if reportTerminal {
            report(error == nil ? .completed : .failed, error: error?.localizedDescription)
        }
    }

    private func handle(_ state: NWConnection.State) {
        guard !finished else { return }
        switch state {
        case .ready where !ready:
            ready = true
            localPort = Self.localPort(of: connection)
            report(.ready)
            readyCallback(id)
            receiveNext()
        case let .failed(error):
            fail(
                UDPFlowSessionError.directConnectionFailed(error.localizedDescription),
                stage: ready ? .relaying : .setup
            )
        case .cancelled:
            fail(
                UDPFlowSessionError.directConnectionFailed("The connection was cancelled."),
                stage: ready ? .relaying : .setup
            )
        default:
            break
        }
    }

    private func receiveNext() {
        guard !finished, ready, !receiveInFlight, let connection else { return }
        receiveInFlight = true
        connection.receiveMessage { [weak self] data, _, isComplete, error in
            guard let self else { return }
            self.receiveInFlight = false
            if let error {
                self.fail(
                    UDPFlowSessionError.directConnectionFailed(error.localizedDescription),
                    stage: .relaying
                )
                return
            }
            guard isComplete else {
                self.fail(UDPFlowSessionError.incompleteDatagram, stage: .relaying)
                return
            }
            guard let data else {
                self.receiveNext()
                return
            }
            self.byteLedger.recordUpstreamReceived(data.count)
            self.lastPayloadAt = Date()
            self.responseCallback(
                self.id,
                UDPFlowDatagram(payload: data, endpoint: self.endpoint)
            )
        }
    }

    private func fail(_ error: Error, stage: UDPConversationFailureStage) {
        guard !finished else { return }
        failureCallback(id, error, stage)
    }

    private func report(_ state: AppRoutingRelayState, error: String? = nil) {
        reporter.report(AppRoutingRelaySnapshot(
            state: state,
            uploadBytes: byteLedger.uploadBytes,
            downloadBytes: byteLedger.downloadBytes,
            error: error,
            note: note,
            localPort: localPort,
            effectiveAction: .direct,
            uploadDatagrams: uploadDatagrams,
            downloadDatagrams: downloadDatagrams,
            droppedDatagrams: 0,
            lastPayloadAt: lastPayloadAt
        ))
    }

    private static func localPort(of connection: NWConnection?) -> UInt16? {
        guard case let .hostPort(_, port) = connection?.currentPath?.localEndpoint else {
            return nil
        }
        return port.rawValue
    }

    private static func increment(_ value: UInt64) -> UInt64 {
        value == .max ? .max : value + 1
    }
}

/// One SOCKS5 UDP association per destination. This prevents Mihomo's UDP NAT
/// mapping and route evidence from conflating several destinations that came
/// from the same application socket.
private final class MihomoUDPConversation: UDPConversation, @unchecked Sendable {
    let id = UUID()
    let activityIdentifier: UUID
    let endpoint: SOCKS5Endpoint

    private let queue: DispatchQueue
    private let proxy: ProviderSOCKSConfiguration
    private let reporter: AppRoutingRelayActivityReporter
    private let readyCallback: @Sendable (UUID) -> Void
    private let responseCallback: @Sendable (UUID, UDPFlowDatagram) -> Void
    private let failureCallback:
        @Sendable (UUID, Error, UDPConversationFailureStage) -> Void
    private var controlConnection: NWConnection?
    private var udpConnection: NWConnection?
    private var startupTimeout: DispatchWorkItem?
    private var methodDecoder = SOCKS5MethodSelectionDecoder()
    private var authenticationDecoder = SOCKS5UsernamePasswordResponseDecoder()
    private var commandDecoder = SOCKS5CommandReplyDecoder()
    private var handshakeStarted = false
    private var ready = false
    private var receiveInFlight = false
    private var finished = false
    private var byteLedger = UDPRelayByteLedger()
    private var uploadDatagrams: UInt64 = 0
    private var downloadDatagrams: UInt64 = 0
    private var localPort: UInt16?
    private var lastPayloadAt: Date?

    init(
        queue: DispatchQueue,
        activityIdentifier: UUID,
        endpoint: SOCKS5Endpoint,
        proxy: ProviderSOCKSConfiguration,
        observer: @escaping @Sendable (AppRoutingRelaySnapshot) -> Void,
        ready: @escaping @Sendable (UUID) -> Void,
        response: @escaping @Sendable (UUID, UDPFlowDatagram) -> Void,
        failure: @escaping @Sendable (UUID, Error, UDPConversationFailureStage) -> Void
    ) {
        self.queue = queue
        self.activityIdentifier = activityIdentifier
        self.endpoint = endpoint
        self.proxy = proxy
        readyCallback = ready
        responseCallback = response
        failureCallback = failure
        reporter = AppRoutingRelayActivityReporter(queue: queue, observer: observer)
    }

    func start() {
        guard !finished else { return }
        report(.connecting)
        let connection = NWConnection(
            host: proxy.networkHost,
            port: proxy.networkPort,
            using: .tcp
        )
        controlConnection = connection
        connection.stateUpdateHandler = { [weak self] state in
            guard let self else { return }
            self.handleControlState(state)
        }
        let timeout = DispatchWorkItem { [weak self] in
            self?.fail(
                UDPFlowSessionError.mihomoSetupFailed("The association timed out."),
                stage: .setup
            )
        }
        startupTimeout = timeout
        queue.asyncAfter(deadline: .now() + .seconds(10), execute: timeout)
        connection.start(queue: queue)
    }

    func send(
        _ payload: Data,
        completion: @escaping @Sendable (Error?) -> Void
    ) {
        guard !finished, ready, let udpConnection else {
            completion(UDPFlowSessionError.mihomoRelayFailed("The UDP relay is not ready."))
            return
        }
        do {
            let wire = try SOCKS5Codec.encodeUDPDatagram(
                SOCKS5UDPDatagram(destination: endpoint, payload: payload)
            )
            udpConnection.send(
                content: wire,
                contentContext: .defaultMessage,
                isComplete: true,
                completion: .contentProcessed { [weak self] error in
                    guard let self else { return }
                    if let error {
                        let failure = UDPFlowSessionError.mihomoRelayFailed(
                            error.localizedDescription
                        )
                        completion(failure)
                        self.fail(failure, stage: .relaying)
                        return
                    }
                    self.byteLedger.recordUpstreamAccepted(payload.count)
                    self.uploadDatagrams = Self.increment(self.uploadDatagrams)
                    self.lastPayloadAt = Date()
                    self.report(.relaying)
                    completion(nil)
                }
            )
        } catch {
            completion(error)
            fail(error, stage: .relaying)
        }
    }

    func responseDelivered(bytes: Int) {
        guard !finished else { return }
        byteLedger.recordApplicationDelivered(bytes)
        downloadDatagrams = Self.increment(downloadDatagrams)
        lastPayloadAt = Date()
        report(.relaying)
    }

    func resumeReceiving() {
        receiveNext()
    }

    func stop(error: Error?, reportTerminal: Bool) {
        guard !finished else { return }
        finished = true
        startupTimeout?.cancel()
        startupTimeout = nil
        controlConnection?.stateUpdateHandler = nil
        controlConnection?.cancel()
        controlConnection = nil
        udpConnection?.stateUpdateHandler = nil
        udpConnection?.cancel()
        udpConnection = nil
        if reportTerminal {
            report(error == nil ? .completed : .failed, error: error?.localizedDescription)
        }
    }

    private func handleControlState(_ state: NWConnection.State) {
        guard !finished else { return }
        switch state {
        case .ready where !handshakeStarted:
            handshakeStarted = true
            beginHandshake()
        case let .failed(error):
            fail(
                ready
                    ? UDPFlowSessionError.mihomoRelayFailed(error.localizedDescription)
                    : UDPFlowSessionError.mihomoSetupFailed(error.localizedDescription),
                stage: ready ? .relaying : .setup
            )
        case .cancelled:
            fail(
                ready
                    ? UDPFlowSessionError.mihomoRelayFailed("The control connection closed.")
                    : UDPFlowSessionError.mihomoSetupFailed("The control connection closed."),
                stage: ready ? .relaying : .setup
            )
        default:
            break
        }
    }

    private func beginHandshake() {
        do {
            let negotiator = SOCKS5ClientAuthenticationNegotiator(
                credentials: proxy.credentials
            )
            try sendControl(negotiator.greeting()) { [weak self] in
                self?.receiveMethodSelection(negotiator: negotiator)
            }
        } catch {
            fail(error, stage: .setup)
        }
    }

    private func receiveMethodSelection(
        negotiator: SOCKS5ClientAuthenticationNegotiator
    ) {
        receiveControlChunk { [weak self] data in
            guard let self else { return }
            do {
                guard let selection = try self.methodDecoder.append(data) else {
                    self.receiveMethodSelection(negotiator: negotiator)
                    return
                }
                switch try negotiator.handle(selection) {
                case .authenticated:
                    self.sendAssociateRequest()
                case let .sendUsernamePassword(request):
                    try self.sendControl(request) { [weak self] in
                        self?.receiveAuthenticationResponse()
                    }
                }
            } catch {
                self.fail(error, stage: .setup)
            }
        }
    }

    private func receiveAuthenticationResponse() {
        receiveControlChunk { [weak self] data in
            guard let self else { return }
            do {
                guard let response = try self.authenticationDecoder.append(data) else {
                    self.receiveAuthenticationResponse()
                    return
                }
                try response.requireSuccess()
                self.sendAssociateRequest()
            } catch {
                self.fail(error, stage: .setup)
            }
        }
    }

    private func sendAssociateRequest() {
        do {
            let unspecified = SOCKS5Endpoint(
                address: SOCKS5Address(ipAddress: try IPAddress("0.0.0.0")),
                port: 0
            )
            let request = try SOCKS5CommandRequest(
                command: .udpAssociate,
                endpoint: unspecified
            )
            try sendControl(SOCKS5Codec.encodeCommandRequest(request)) { [weak self] in
                self?.receiveAssociateResponse()
            }
        } catch {
            fail(error, stage: .setup)
        }
    }

    private func receiveAssociateResponse() {
        receiveControlChunk { [weak self] data in
            guard let self else { return }
            do {
                guard let response = try self.commandDecoder.append(data) else {
                    self.receiveAssociateResponse()
                    return
                }
                let bound = try response.requireSuccess()
                guard self.commandDecoder.remainingData.isEmpty else {
                    throw UDPFlowRelayError.unexpectedControlData
                }
                let relay = try self.normalizedRelayEndpoint(bound)
                self.monitorControlConnection()
                try self.startUDPConnection(to: relay)
            } catch {
                self.fail(error, stage: .setup)
            }
        }
    }

    private func normalizedRelayEndpoint(
        _ endpoint: SOCKS5Endpoint
    ) throws -> SOCKS5Endpoint {
        guard endpoint.port > 0 else { throw UDPFlowRelayError.invalidRelayEndpoint }
        guard endpoint.address.ipAddress?.isUnspecified == true else { return endpoint }
        if let address = try? IPAddress(proxy.host) {
            return SOCKS5Endpoint(
                address: SOCKS5Address(ipAddress: address),
                port: endpoint.port
            )
        }
        return SOCKS5Endpoint(
            address: try SOCKS5Address(domain: proxy.host),
            port: endpoint.port
        )
    }

    private func startUDPConnection(to endpoint: SOCKS5Endpoint) throws {
        guard let host = endpoint.address.ipAddress?.presentation
                ?? endpoint.address.domain,
              let port = NWEndpoint.Port(rawValue: endpoint.port) else {
            throw UDPFlowRelayError.invalidRelayEndpoint
        }
        let connection = NWConnection(
            host: NWEndpoint.Host(host),
            port: port,
            using: .udp
        )
        udpConnection = connection
        connection.stateUpdateHandler = { [weak self] state in
            guard let self else { return }
            self.handleUDPState(state)
        }
        connection.start(queue: queue)
    }

    private func handleUDPState(_ state: NWConnection.State) {
        guard !finished else { return }
        switch state {
        case .ready where !ready:
            ready = true
            startupTimeout?.cancel()
            startupTimeout = nil
            localPort = Self.localPort(of: udpConnection)
            report(.ready)
            readyCallback(id)
            receiveNext()
        case let .failed(error):
            fail(
                ready
                    ? UDPFlowSessionError.mihomoRelayFailed(error.localizedDescription)
                    : UDPFlowSessionError.mihomoSetupFailed(error.localizedDescription),
                stage: ready ? .relaying : .setup
            )
        case .cancelled:
            fail(
                ready
                    ? UDPFlowSessionError.mihomoRelayFailed("The UDP relay closed.")
                    : UDPFlowSessionError.mihomoSetupFailed("The UDP relay closed."),
                stage: ready ? .relaying : .setup
            )
        default:
            break
        }
    }

    private func sendControl(
        _ data: Data,
        then next: @escaping @Sendable () -> Void
    ) throws {
        guard let controlConnection else { throw UDPFlowRelayError.controlConnectionClosed }
        controlConnection.send(
            content: data,
            completion: .contentProcessed { [weak self] error in
                guard let self else { return }
                if let error {
                    self.fail(
                        UDPFlowSessionError.mihomoSetupFailed(error.localizedDescription),
                        stage: .setup
                    )
                } else if !self.finished {
                    next()
                }
            }
        )
    }

    private func receiveControlChunk(
        _ consume: @escaping @Sendable (Data) -> Void
    ) {
        guard let controlConnection else {
            fail(UDPFlowRelayError.controlConnectionClosed, stage: .setup)
            return
        }
        controlConnection.receive(
            minimumIncompleteLength: 1,
            maximumLength: 64 * 1_024
        ) { [weak self] data, _, isComplete, error in
            guard let self else { return }
            if let error {
                self.fail(
                    UDPFlowSessionError.mihomoSetupFailed(error.localizedDescription),
                    stage: .setup
                )
            } else if let data, !data.isEmpty {
                consume(data)
            } else if isComplete {
                self.fail(UDPFlowRelayError.controlConnectionClosed, stage: .setup)
            } else {
                self.receiveControlChunk(consume)
            }
        }
    }

    private func monitorControlConnection() {
        guard !finished, let controlConnection else { return }
        controlConnection.receive(
            minimumIncompleteLength: 1,
            maximumLength: 1
        ) { [weak self] data, _, isComplete, error in
            guard let self else { return }
            if let error {
                self.fail(
                    UDPFlowSessionError.mihomoRelayFailed(error.localizedDescription),
                    stage: .relaying
                )
            } else if let data, !data.isEmpty {
                self.fail(UDPFlowRelayError.unexpectedControlData, stage: .relaying)
            } else if isComplete {
                self.fail(UDPFlowRelayError.controlConnectionClosed, stage: .relaying)
            } else {
                self.monitorControlConnection()
            }
        }
    }

    private func receiveNext() {
        guard !finished, ready, !receiveInFlight, let udpConnection else { return }
        receiveInFlight = true
        udpConnection.receiveMessage { [weak self] data, _, isComplete, error in
            guard let self else { return }
            self.receiveInFlight = false
            if let error {
                self.fail(
                    UDPFlowSessionError.mihomoRelayFailed(error.localizedDescription),
                    stage: .relaying
                )
                return
            }
            guard isComplete else {
                self.fail(UDPFlowSessionError.incompleteDatagram, stage: .relaying)
                return
            }
            guard let data else {
                self.receiveNext()
                return
            }
            do {
                let decoded = try SOCKS5Codec.decodeUDPDatagram(data)
                self.byteLedger.recordUpstreamReceived(decoded.payload.count)
                self.lastPayloadAt = Date()
                self.responseCallback(
                    self.id,
                    UDPFlowDatagram(
                        payload: decoded.payload,
                        endpoint: decoded.destination
                    )
                )
            } catch {
                self.fail(error, stage: .relaying)
            }
        }
    }

    private func fail(_ error: Error, stage: UDPConversationFailureStage) {
        guard !finished else { return }
        failureCallback(id, error, stage)
    }

    private func report(_ state: AppRoutingRelayState, error: String? = nil) {
        reporter.report(AppRoutingRelaySnapshot(
            state: state,
            uploadBytes: byteLedger.uploadBytes,
            downloadBytes: byteLedger.downloadBytes,
            error: error,
            localPort: localPort,
            uploadDatagrams: uploadDatagrams,
            downloadDatagrams: downloadDatagrams,
            droppedDatagrams: 0,
            lastPayloadAt: lastPayloadAt
        ))
    }

    private static func localPort(of connection: NWConnection?) -> UInt16? {
        guard case let .hostPort(_, port) = connection?.currentPath?.localEndpoint else {
            return nil
        }
        return port.rawValue
    }

    private static func increment(_ value: UInt64) -> UInt64 {
        value == .max ? .max : value + 1
    }
}

final class UDPFlowSessionRegistry: @unchecked Sendable {
    static let maximumActiveConversations = 1_024
    private static let maximumActiveSessions = 256

    private let lock = NSLock()
    private var generation = UUID()
    private var sessions: [UUID: UDPFlowSession] = [:]
    private var activeConversations = 0

    @discardableResult
    func start(
        id: UUID,
        flow: NEAppProxyUDPFlow,
        initialPlan: UDPFlowInterceptionPlan,
        planner: @escaping @Sendable (SOCKS5Endpoint) -> UDPFlowInterceptionPlan,
        revisionProvider: @escaping @Sendable () -> UInt64,
        activitySink: @escaping @Sendable (AppRoutingActivity) -> Void,
        observerFactory:
            @escaping @Sendable (UUID) -> @Sendable (AppRoutingRelaySnapshot) -> Void
    ) -> Bool {
        lock.lock()
        guard sessions.count < Self.maximumActiveSessions else {
            lock.unlock()
            return false
        }
        let expectedGeneration = generation
        let session = UDPFlowSession(
            id: id,
            flow: flow,
            initialPlan: initialPlan,
            planner: planner,
            revisionProvider: revisionProvider,
            activitySink: activitySink,
            observerFactory: observerFactory,
            acquireConversationAdmission: { [weak self] in
                self?.acquireConversation() ?? false
            },
            releaseConversationAdmission: { [weak self] in
                self?.releaseConversation()
            },
            completion: { [weak self] identifier in
                self?.remove(identifier, generation: expectedGeneration)
            }
        )
        sessions[id] = session
        lock.unlock()
        session.start()
        return true
    }

    func cancelAll() {
        lock.lock()
        generation = UUID()
        let current = Array(sessions.values)
        sessions.removeAll(keepingCapacity: false)
        lock.unlock()
        current.forEach { $0.cancel() }
    }

    private func acquireConversation() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        guard activeConversations < Self.maximumActiveConversations else { return false }
        activeConversations += 1
        return true
    }

    private func releaseConversation() {
        lock.lock()
        activeConversations = max(0, activeConversations - 1)
        lock.unlock()
    }

    private func remove(_ identifier: UUID, generation expectedGeneration: UUID) {
        lock.lock()
        if generation == expectedGeneration {
            sessions.removeValue(forKey: identifier)
        }
        lock.unlock()
    }
}

/// Startup probe used by the DNS provider. Success means the private listener
/// accepted the configured authentication and created a SOCKS5 UDP association;
/// no DNS destination or payload is sent by the probe.
final class MihomoUDPAssociationProbe: @unchecked Sendable {
    private let queue = DispatchQueue(label: "local.harry.myproxy.socks-udp-probe")
    private var conversation: MihomoUDPConversation?
    private var completion: (@Sendable (Error?) -> Void)?
    private var cancelled = false

    func start(
        proxy: ProviderSOCKSConfiguration,
        completion: @escaping @Sendable (Error?) -> Void
    ) {
        queue.async { [self] in
            guard self.completion == nil else { return }
            guard !cancelled else {
                completion(UDPFlowSessionError.cancelled)
                return
            }
            self.completion = completion
            do {
                let endpoint = SOCKS5Endpoint(
                    address: SOCKS5Address(ipAddress: try IPAddress("127.0.0.1")),
                    port: 53
                )
                let conversation = MihomoUDPConversation(
                    queue: queue,
                    activityIdentifier: UUID(),
                    endpoint: endpoint,
                    proxy: proxy,
                    observer: { _ in },
                    ready: { [weak self] _ in self?.finish(nil) },
                    response: { _, _ in },
                    failure: { [weak self] _, error, _ in self?.finish(error) }
                )
                self.conversation = conversation
                conversation.start()
            } catch {
                finish(error)
            }
        }
    }

    func cancel() {
        queue.async { [self] in
            cancelled = true
            finish(UDPFlowSessionError.cancelled)
        }
    }

    private func finish(_ error: Error?) {
        guard let completion else { return }
        self.completion = nil
        conversation?.stop(error: nil, reportTerminal: false)
        conversation = nil
        completion(error)
    }
}

private extension UDPFlowInterceptionPlan {
    var configurationRevision: UInt64 { activity.configurationRevision }
}
