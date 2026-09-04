import Foundation
import MyproxyNetworkShared
@preconcurrency import Network
@preconcurrency import NetworkExtension

enum DirectUDPFlowRelayError: Error, LocalizedError, Sendable {
    case cancelled
    case idleTimedOut
    case flowOpenFailed(String)
    case flowReadFailed(String)
    case flowWriteFailed(String)
    case upstreamFailed(String)
    case unsupportedEndpoint
    case datagramTooLarge(limit: Int, actual: Int)
    case unexpectedDestination
    case tooManyDestinations(limit: Int)
    case responseQueueFull(maximumDatagrams: Int, maximumBytes: Int)
    case incompleteUpstreamDatagram

    var errorDescription: String? {
        switch self {
        case .cancelled:
            "The direct UDP relay was cancelled because App Routing stopped or slept."
        case .idleTimedOut:
            "The direct UDP relay exceeded its idle timeout."
        case let .flowOpenFailed(message):
            "The intercepted UDP flow could not be opened: \(message)"
        case let .flowReadFailed(message):
            "Reading the intercepted UDP flow failed: \(message)"
        case let .flowWriteFailed(message):
            "Writing the intercepted UDP flow failed: \(message)"
        case let .upstreamFailed(message):
            "The direct UDP connection failed: \(message)"
        case .unsupportedEndpoint:
            "The direct UDP datagram endpoint is unsupported."
        case let .datagramTooLarge(limit, actual):
            "The intercepted UDP datagram is \(actual) bytes; limit is \(limit)."
        case .unexpectedDestination:
            "The UDP flow changed destinations after its routing decision; myproxy stopped it rather than applying the wrong IP or port rule."
        case let .tooManyDestinations(limit):
            "The direct UDP flow exceeded its safety limit of \(limit) remote destinations."
        case let .responseQueueFull(maximumDatagrams, maximumBytes):
            "The direct UDP response queue reached its safety limit of \(maximumDatagrams) datagrams or \(maximumBytes) bytes."
        case .incompleteUpstreamDatagram:
            "The direct UDP connection returned an incomplete datagram."
        }
    }
}

/// Relays one intercepted UDP flow to its original per-datagram destinations.
///
/// An `NEAppProxyUDPFlow` can contain several remote endpoints, so each unique
/// destination receives its own connected `NWConnection`. Application reads
/// are serialized behind upstream sends, and all upstream responses share one
/// bounded write queue back into the app flow.
final class DirectUDPFlowRelay: @unchecked Sendable {
    let id = UUID()

    private enum Limits {
        static let idleTimeout: DispatchTimeInterval = .seconds(120)
        static let maximumDestinations = 32
        static let maximumQueuedResponses = 256
        static let maximumQueuedResponseBytes = 4 * 1_024 * 1_024
    }

    private let flow: NEAppProxyUDPFlow
    private let expectedDestination: SOCKS5Endpoint
    private let relayNote: String?
    private let queue: DispatchQueue
    private let completion: @Sendable (UUID) -> Void
    private let activityReporter: AppRoutingRelayActivityReporter

    private var connections: [SOCKS5Endpoint: NWConnection] = [:]
    private var receiveInFlightEndpoints: Set<SOCKS5Endpoint> = []
    private var pausedReceiveEndpoints: Set<SOCKS5Endpoint> = []
    private var pendingFlowWrites: [UDPFlowDatagram] = []
    private var responseBudget = UDPRelayQueueBudget(
        maximumDatagrams: Limits.maximumQueuedResponses,
        maximumBytes: Limits.maximumQueuedResponseBytes
    )
    private var flowWriteInFlight = false
    private var idleTimeout: DispatchWorkItem?
    private var idleGeneration: UInt64 = 0
    private var openedFlow = false
    private var finished = false
    private var byteLedger = UDPRelayByteLedger()
    private var relayLocalPort: UInt16?

    init(
        flow: NEAppProxyUDPFlow,
        expectedDestination: SOCKS5Endpoint,
        relayNote: String? = nil,
        activityObserver: @escaping @Sendable (AppRoutingRelaySnapshot) -> Void,
        completion: @escaping @Sendable (UUID) -> Void
    ) {
        self.flow = flow
        self.expectedDestination = expectedDestination
        self.relayNote = relayNote
        self.completion = completion
        let queue = DispatchQueue(label: "local.harry.myproxy.direct-udp-relay.\(id.uuidString)")
        self.queue = queue
        activityReporter = AppRoutingRelayActivityReporter(
            queue: queue,
            observer: activityObserver
        )
    }

    func start() {
        queue.async { [self] in
            guard !finished else { return }
            report(.connecting)
            AppProxyFlowCompatibility.open(flow) { [weak self] error in
                guard let self else { return }
                self.queue.async {
                    if let error {
                        self.finish(error: DirectUDPFlowRelayError.flowOpenFailed(
                            error.localizedDescription
                        ))
                        return
                    }
                    self.openedFlow = true
                    self.report(.ready)
                    self.resetIdleTimeout()
                    self.readFromFlow()
                }
            }
        }
    }

    func cancel() {
        queue.async { [self] in
            finish(error: DirectUDPFlowRelayError.cancelled)
        }
    }

    private func readFromFlow() {
        guard !finished else { return }
        UDPAppProxyFlowCompatibility.read(from: flow) { [weak self] datagrams, error in
            guard let self else { return }
            self.queue.async {
                if let error {
                    self.finish(error: DirectUDPFlowRelayError.flowReadFailed(
                        error.localizedDescription
                    ))
                    return
                }
                guard let datagrams, !datagrams.isEmpty else {
                    self.finish(error: nil)
                    return
                }
                do {
                    try self.validate(datagrams)
                    for datagram in datagrams {
                        self.byteLedger.recordApplicationRead(datagram.payload.count)
                    }
                    self.resetIdleTimeout()
                    self.sendToUpstream(datagrams, index: 0)
                } catch {
                    self.finish(error: error)
                }
            }
        }
    }

    private func validate(_ datagrams: [UDPFlowDatagram]) throws {
        for datagram in datagrams {
            guard datagram.endpoint == expectedDestination else {
                throw DirectUDPFlowRelayError.unexpectedDestination
            }
            guard datagram.payload.count <= SOCKS5Limits.maximumUDPDatagramBytes else {
                throw DirectUDPFlowRelayError.datagramTooLarge(
                    limit: SOCKS5Limits.maximumUDPDatagramBytes,
                    actual: datagram.payload.count
                )
            }
        }
    }

    private func sendToUpstream(_ datagrams: [UDPFlowDatagram], index: Int) {
        guard !finished else { return }
        guard index < datagrams.count else {
            readFromFlow()
            return
        }

        let datagram = datagrams[index]
        do {
            let connection = try connection(for: datagram.endpoint)
            connection.send(
                content: datagram.payload,
                contentContext: .defaultMessage,
                isComplete: true,
                completion: .contentProcessed { [weak self] error in
                    guard let self else { return }
                    self.queue.async {
                        if let error {
                            self.finish(error: DirectUDPFlowRelayError.upstreamFailed(
                                error.localizedDescription
                            ))
                        } else if !self.finished {
                            self.byteLedger.recordUpstreamAccepted(datagram.payload.count)
                            self.report(.relaying)
                            self.resetIdleTimeout()
                            self.sendToUpstream(datagrams, index: index + 1)
                        }
                    }
                }
            )
        } catch {
            finish(error: error)
        }
    }

    private func connection(for endpoint: SOCKS5Endpoint) throws -> NWConnection {
        if let connection = connections[endpoint] { return connection }
        guard connections.count < Limits.maximumDestinations else {
            throw DirectUDPFlowRelayError.tooManyDestinations(
                limit: Limits.maximumDestinations
            )
        }
        guard endpoint.port > 0,
              let port = NWEndpoint.Port(rawValue: endpoint.port),
              let host = Self.host(endpoint.address) else {
            throw DirectUDPFlowRelayError.unsupportedEndpoint
        }

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
        connections[endpoint] = connection
        connection.stateUpdateHandler = { [weak self] state in
            guard let self else { return }
            self.queue.async {
                self.handleConnectionState(state, endpoint: endpoint, connection: connection)
            }
        }
        connection.start(queue: queue)
        return connection
    }

    private func handleConnectionState(
        _ state: NWConnection.State,
        endpoint: SOCKS5Endpoint,
        connection: NWConnection
    ) {
        guard !finished, connections[endpoint] === connection else { return }
        switch state {
        case .ready:
            if relayLocalPort == nil {
                relayLocalPort = Self.localPort(of: connection)
                report(.relaying)
            }
            startReceiving(from: endpoint, connection: connection)
        case let .failed(error):
            finish(error: DirectUDPFlowRelayError.upstreamFailed(error.localizedDescription))
        case .cancelled:
            finish(error: DirectUDPFlowRelayError.upstreamFailed(
                "The UDP connection was cancelled."
            ))
        default:
            break
        }
    }

    private func startReceiving(
        from endpoint: SOCKS5Endpoint,
        connection: NWConnection
    ) {
        receiveNext(from: endpoint, connection: connection)
    }

    private func receiveNext(
        from endpoint: SOCKS5Endpoint,
        connection: NWConnection
    ) {
        guard !finished,
              connections[endpoint] === connection,
              !receiveInFlightEndpoints.contains(endpoint) else { return }
        guard responseBudget.datagramCount
                < Limits.maximumQueuedResponses - Limits.maximumDestinations,
              responseBudget.canReserve(bytes: SOCKS5Limits.maximumUDPDatagramBytes)
        else {
            pausedReceiveEndpoints.insert(endpoint)
            return
        }
        pausedReceiveEndpoints.remove(endpoint)
        receiveInFlightEndpoints.insert(endpoint)
        connection.receiveMessage { [weak self] data, _, isComplete, error in
            guard let self else { return }
            self.queue.async {
                self.receiveInFlightEndpoints.remove(endpoint)
                if let error {
                    self.finish(error: DirectUDPFlowRelayError.upstreamFailed(
                        error.localizedDescription
                    ))
                    return
                }
                guard isComplete else {
                    self.finish(error: DirectUDPFlowRelayError.incompleteUpstreamDatagram)
                    return
                }
                if let data {
                    self.byteLedger.recordUpstreamReceived(data.count)
                    self.resetIdleTimeout()
                    self.enqueueFlowWrite(UDPFlowDatagram(payload: data, endpoint: endpoint))
                }
                if !self.finished {
                    self.receiveNext(from: endpoint, connection: connection)
                }
            }
        }
    }

    private func enqueueFlowWrite(_ datagram: UDPFlowDatagram) {
        guard responseBudget.reserve(bytes: datagram.payload.count) else {
            finish(error: DirectUDPFlowRelayError.responseQueueFull(
                maximumDatagrams: Limits.maximumQueuedResponses,
                maximumBytes: Limits.maximumQueuedResponseBytes
            ))
            return
        }
        pendingFlowWrites.append(datagram)
        writeNextToFlowIfNeeded()
    }

    private func writeNextToFlowIfNeeded() {
        guard !finished,
              !flowWriteInFlight,
              let datagram = pendingFlowWrites.first else { return }
        flowWriteInFlight = true
        UDPAppProxyFlowCompatibility.write(datagram, to: flow) { [weak self] error in
            guard let self else { return }
            self.queue.async {
                guard !self.finished else { return }
                self.flowWriteInFlight = false
                if let error {
                    self.finish(error: DirectUDPFlowRelayError.flowWriteFailed(
                        error.localizedDescription
                    ))
                    return
                }
                _ = self.pendingFlowWrites.removeFirst()
                self.responseBudget.release(bytes: datagram.payload.count)
                self.byteLedger.recordApplicationDelivered(datagram.payload.count)
                self.report(.relaying)
                self.resetIdleTimeout()
                self.resumePausedReceives()
                self.writeNextToFlowIfNeeded()
            }
        }
    }

    private func resumePausedReceives() {
        for endpoint in Array(pausedReceiveEndpoints) {
            guard let connection = connections[endpoint] else {
                pausedReceiveEndpoints.remove(endpoint)
                continue
            }
            receiveNext(from: endpoint, connection: connection)
        }
    }

    private func resetIdleTimeout() {
        idleTimeout?.cancel()
        idleGeneration &+= 1
        let generation = idleGeneration
        let work = DispatchWorkItem { [weak self] in
            guard let self, self.idleGeneration == generation else { return }
            self.finish(error: DirectUDPFlowRelayError.idleTimedOut)
        }
        idleTimeout = work
        queue.asyncAfter(deadline: .now() + Limits.idleTimeout, execute: work)
    }

    private func finish(error: Error?) {
        guard !finished else { return }
        finished = true
        idleTimeout?.cancel()
        idleTimeout = nil
        for connection in connections.values {
            connection.stateUpdateHandler = nil
            connection.cancel()
        }
        connections.removeAll(keepingCapacity: false)
        receiveInFlightEndpoints.removeAll(keepingCapacity: false)
        pausedReceiveEndpoints.removeAll(keepingCapacity: false)
        pendingFlowWrites.removeAll(keepingCapacity: false)

        if let error {
            flow.closeReadWithError(error)
            flow.closeWriteWithError(error)
        } else if openedFlow {
            flow.closeReadWithError(nil)
            flow.closeWriteWithError(nil)
        }
        report(
            error == nil ? .completed : .failed,
            error: error?.localizedDescription
        )
        completion(id)
    }

    private func report(_ state: AppRoutingRelayState, error: String? = nil) {
        activityReporter.report(AppRoutingRelaySnapshot(
            state: state,
            uploadBytes: byteLedger.uploadBytes,
            downloadBytes: byteLedger.downloadBytes,
            error: error,
            note: relayNote,
            localPort: relayLocalPort,
            effectiveAction: .direct
        ))
    }

    private static func host(_ address: SOCKS5Address) -> String? {
        address.ipAddress?.presentation ?? address.domain
    }

    private static func localPort(of connection: NWConnection) -> UInt16? {
        guard case let .hostPort(_, port) = connection.currentPath?.localEndpoint else {
            return nil
        }
        return port.rawValue
    }
}
