import Foundation
import MyproxyNetworkShared
@preconcurrency import Network
@preconcurrency import NetworkExtension

enum DirectTCPFlowRelayError: Error, LocalizedError, Sendable {
    case cancelled
    case invalidDestination
    case connectionTimedOut
    case upstreamFailed(String)
    case flowOpenFailed(String)
    case flowReadFailed(String)
    case flowWriteFailed(String)

    var errorDescription: String? {
        switch self {
        case .cancelled:
            "The direct TCP relay was cancelled because App Routing stopped or slept."
        case .invalidDestination:
            "The direct TCP destination is invalid."
        case .connectionTimedOut:
            "The direct TCP connection timed out."
        case let .upstreamFailed(message):
            "The direct TCP connection failed: \(message)"
        case let .flowOpenFailed(message):
            "The intercepted TCP flow could not be opened: \(message)"
        case let .flowReadFailed(message):
            "Reading the intercepted TCP flow failed: \(message)"
        case let .flowWriteFailed(message):
            "Writing the intercepted TCP flow failed: \(message)"
        }
    }
}

/// Relays an intercepted application TCP flow to its original destination.
/// There is never more than one in-flight chunk in either direction, bounding
/// memory and applying end-to-end backpressure. Visible byte counters advance
/// only after the destination accepts a send or the application accepts a
/// write.
final class DirectTCPFlowRelay: @unchecked Sendable {
    let id = UUID()

    private let flow: NEAppProxyTCPFlow
    private let destination: SOCKS5Endpoint
    private let relayNote: String?
    private let queue: DispatchQueue
    private let completion: @Sendable (UUID) -> Void
    private let activityReporter: AppRoutingRelayActivityReporter

    private var connection: NWConnection?
    private var connectionTimeout: DispatchWorkItem?
    private var openedFlow = false
    private var openingFlow = false
    private var halfCloseState = TCPRelayHalfCloseState()
    private var backpressureState = TCPRelayBackpressureState()
    private var byteLedger = TCPRelayByteLedger()
    private var relayLocalPort: UInt16?
    private var finished = false

    init(
        flow: NEAppProxyTCPFlow,
        destination: SOCKS5Endpoint,
        relayNote: String? = nil,
        activityObserver: @escaping @Sendable (AppRoutingRelaySnapshot) -> Void,
        completion: @escaping @Sendable (UUID) -> Void
    ) {
        self.flow = flow
        self.destination = destination
        self.relayNote = relayNote
        self.completion = completion
        let queue = DispatchQueue(label: "local.harry.myproxy.direct-tcp-relay.\(id.uuidString)")
        self.queue = queue
        activityReporter = AppRoutingRelayActivityReporter(
            queue: queue,
            observer: activityObserver
        )
    }

    func start() {
        queue.async { [self] in
            guard !finished else { return }
            guard let host = destination.networkHost,
                  let port = destination.networkPort
            else {
                finish(error: DirectTCPFlowRelayError.invalidDestination)
                return
            }

            let parameters = NWParameters.tcp
            // Direct means the original network path, not the process-wide
            // HTTP/SOCKS proxy. This also prevents a System Proxy feedback loop.
            parameters.preferNoProxies = true
            if #available(macOS 15.0, *) {
                // This official API carries the source application's metadata
                // to any subsequent transparent providers. The Swift overlay
                // exposes it on macOS 15 while the target remains macOS 14.
                flow.setMetadata(on: parameters)
            }

            report(.connecting)
            let connection = NWConnection(host: host, port: port, using: parameters)
            self.connection = connection
            connection.stateUpdateHandler = { [weak self] state in
                guard let self else { return }
                self.handleConnectionState(state)
            }
            scheduleConnectionTimeout()
            connection.start(queue: queue)
        }
    }

    func cancel() {
        queue.async { [self] in
            finish(error: DirectTCPFlowRelayError.cancelled)
        }
    }

    private func handleConnectionState(_ state: NWConnection.State) {
        guard !finished else { return }
        switch state {
        case .ready:
            guard !openingFlow, !openedFlow else { return }
            openingFlow = true
            connectionTimeout?.cancel()
            connectionTimeout = nil
            relayLocalPort = Self.localPort(of: connection)
            openFlow()
        case let .failed(error):
            finish(error: DirectTCPFlowRelayError.upstreamFailed(error.localizedDescription))
        case .cancelled:
            finish(error: nil)
        default:
            break
        }
    }

    private func scheduleConnectionTimeout() {
        let work = DispatchWorkItem { [weak self] in
            self?.finish(error: DirectTCPFlowRelayError.connectionTimedOut)
        }
        connectionTimeout = work
        queue.asyncAfter(deadline: .now() + .seconds(30), execute: work)
    }

    private func openFlow() {
        AppProxyFlowCompatibility.open(flow) { [weak self] error in
            guard let self else { return }
            self.queue.async {
                if let error {
                    self.finish(error: DirectTCPFlowRelayError.flowOpenFailed(
                        error.localizedDescription
                    ))
                    return
                }
                self.openedFlow = true
                self.report(.ready)
                self.readFromFlow()
                self.readFromUpstream()
            }
        }
    }

    private func readFromFlow() {
        guard !finished,
              !halfCloseState.appReadEnded,
              backpressureState.begin(.appToUpstream)
        else { return }
        flow.readData { [weak self] data, error in
            guard let self else { return }
            self.queue.async {
                if let error {
                    self.finish(error: DirectTCPFlowRelayError.flowReadFailed(
                        error.localizedDescription
                    ))
                    return
                }
                guard let data, !data.isEmpty else {
                    self.halfCloseState.markAppReadEnded()
                    self.sendUpstreamEOF()
                    return
                }

                self.byteLedger.recordAppRead(data.count)
                guard let connection = self.connection else {
                    self.finish(error: DirectTCPFlowRelayError.upstreamFailed(
                        "The connection disappeared before the payload was sent."
                    ))
                    return
                }
                connection.send(
                    content: data,
                    completion: .contentProcessed { [weak self] error in
                        guard let self else { return }
                        if let error {
                            self.finish(error: DirectTCPFlowRelayError.upstreamFailed(
                                error.localizedDescription
                            ))
                        } else if !self.finished {
                            self.backpressureState.end(.appToUpstream)
                            self.byteLedger.recordUpstreamAccepted(data.count)
                            self.report(.relaying)
                            self.readFromFlow()
                        }
                    }
                )
            }
        }
    }

    private func sendUpstreamEOF() {
        guard let connection else {
            finish(error: DirectTCPFlowRelayError.upstreamFailed(
                "The connection disappeared before the upload half closed."
            ))
            return
        }
        connection.send(
            content: nil,
            contentContext: .defaultMessage,
            isComplete: true,
            completion: .contentProcessed { [weak self] error in
                guard let self else { return }
                if let error {
                    self.finish(error: DirectTCPFlowRelayError.upstreamFailed(
                        error.localizedDescription
                    ))
                } else {
                    self.backpressureState.end(.appToUpstream)
                    self.finishIfBothHalvesClosed()
                }
            }
        )
    }

    private func readFromUpstream() {
        guard !finished,
              !halfCloseState.upstreamReadEnded,
              let connection,
              backpressureState.begin(.upstreamToApp)
        else { return }
        connection.receive(
            minimumIncompleteLength: 1,
            maximumLength: 64 * 1_024
        ) { [weak self] data, _, isComplete, error in
            guard let self else { return }
            if let error {
                self.finish(error: DirectTCPFlowRelayError.upstreamFailed(
                    error.localizedDescription
                ))
                return
            }
            if let data, !data.isEmpty {
                self.byteLedger.recordUpstreamReceived(data.count)
                self.writeToFlow(data) { [weak self] in
                    guard let self else { return }
                    self.backpressureState.end(.upstreamToApp)
                    self.byteLedger.recordAppDelivered(data.count)
                    self.report(.relaying)
                    if isComplete {
                        self.markUpstreamReadFinished()
                    } else {
                        self.readFromUpstream()
                    }
                }
            } else if isComplete {
                self.backpressureState.end(.upstreamToApp)
                self.markUpstreamReadFinished()
            } else {
                self.backpressureState.end(.upstreamToApp)
                self.readFromUpstream()
            }
        }
    }

    private func writeToFlow(_ data: Data, then next: @escaping @Sendable () -> Void) {
        flow.write(data) { [weak self] error in
            guard let self else { return }
            self.queue.async {
                if let error {
                    self.finish(error: DirectTCPFlowRelayError.flowWriteFailed(
                        error.localizedDescription
                    ))
                } else if !self.finished {
                    next()
                }
            }
        }
    }

    private func markUpstreamReadFinished() {
        halfCloseState.markUpstreamReadEnded()
        flow.closeWriteWithError(nil)
        finishIfBothHalvesClosed()
    }

    private func finishIfBothHalvesClosed() {
        if halfCloseState.bothReadHalvesEnded {
            finish(error: nil)
        }
    }

    private func finish(error: Error?) {
        guard !finished else { return }
        finished = true
        connectionTimeout?.cancel()
        connectionTimeout = nil
        connection?.stateUpdateHandler = nil
        connection?.cancel()
        connection = nil

        if let error {
            flow.closeReadWithError(error)
            flow.closeWriteWithError(error)
        } else if openedFlow {
            if !halfCloseState.appReadEnded { flow.closeReadWithError(nil) }
            if !halfCloseState.upstreamReadEnded { flow.closeWriteWithError(nil) }
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

    private static func localPort(of connection: NWConnection?) -> UInt16? {
        guard case let .hostPort(_, port) = connection?.currentPath?.localEndpoint else {
            return nil
        }
        return port.rawValue
    }
}

private extension SOCKS5Endpoint {
    var networkHost: NWEndpoint.Host? {
        if let address = address.ipAddress?.presentation {
            return NWEndpoint.Host(address)
        }
        if let domain = address.domain {
            return NWEndpoint.Host(domain)
        }
        return nil
    }

    var networkPort: NWEndpoint.Port? {
        guard port > 0 else { return nil }
        return NWEndpoint.Port(rawValue: port)
    }
}
