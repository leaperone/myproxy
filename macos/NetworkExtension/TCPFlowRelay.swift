import Foundation
import MyproxyNetworkShared
@preconcurrency import Network
@preconcurrency import NetworkExtension

enum TCPFlowRelayError: Error, LocalizedError, Sendable {
    case cancelled
    case capacityExceeded
    case handshakeTimedOut
    case upstreamClosedDuringHandshake
    case upstreamFailed(String)
    case flowOpenFailed(String)
    case flowReadFailed(String)
    case flowWriteFailed(String)

    var errorDescription: String? {
        switch self {
        case .cancelled:
            "The TCP relay was cancelled because App Routing stopped or slept."
        case .capacityExceeded:
            "myproxy reached the active TCP relay safety limit."
        case .handshakeTimedOut:
            "The local mihomo SOCKS5 handshake timed out."
        case .upstreamClosedDuringHandshake:
            "The local mihomo SOCKS5 listener closed during handshake."
        case let .upstreamFailed(message):
            "The local mihomo SOCKS5 connection failed: \(message)"
        case let .flowOpenFailed(message):
            "The intercepted TCP flow could not be opened: \(message)"
        case let .flowReadFailed(message):
            "Reading the intercepted TCP flow failed: \(message)"
        case let .flowWriteFailed(message):
            "Writing the intercepted TCP flow failed: \(message)"
        }
    }
}

enum TCPFlowRelayExit: Sendable {
    case finished
    case directFallback(String)
}

enum AppProxyFlowCompatibility {
    static func open(
        _ flow: NEAppProxyFlow,
        completion: @escaping @Sendable (Error?) -> Void
    ) {
        if #available(macOS 15.0, *) {
            flow.open(withLocalFlowEndpoint: nil, completionHandler: completion)
            return
        }

        // The macOS 14 Objective-C selector remains public and supported, but
        // Swift 6 hides its deprecated NWHostEndpoint parameter when compiling
        // against the macOS 15+ SDK. Invoke that exact selector only on 14.
        let selector = NSSelectorFromString("openWithLocalEndpoint:completionHandler:")
        let block: @convention(block) (NSError?) -> Void = { error in
            completion(error)
        }
        _ = flow.perform(selector, with: nil, with: block)
    }
}

/// Owns one intercepted TCP flow from the synchronous provider callback until
/// both halves close. Reads are issued only after the previous write completes,
/// providing bounded backpressure in both directions.
final class TCPFlowRelay: @unchecked Sendable {
    let id = UUID()

    private let flow: NEAppProxyTCPFlow
    private let proxy: ProviderSOCKSConfiguration
    private let destination: SOCKS5Endpoint
    private let queue: DispatchQueue
    private let completion: @Sendable (UUID, TCPFlowRelayExit) -> Void
    private let activityReporter: AppRoutingRelayActivityReporter

    private var connection: NWConnection?
    private var handshakeTimeout: DispatchWorkItem?
    private var methodDecoder = SOCKS5MethodSelectionDecoder()
    private var authenticationDecoder = SOCKS5UsernamePasswordResponseDecoder()
    private var commandDecoder = SOCKS5CommandReplyDecoder()
    private var openedFlow = false
    private var handshakeStarted = false
    private var failoverState: TCPRelayFailoverState
    private var halfCloseState = TCPRelayHalfCloseState()
    private var backpressureState = TCPRelayBackpressureState()
    private var finished = false
    private var byteLedger = TCPRelayByteLedger()
    private var relayLocalPort: UInt16?

    init(
        flow: NEAppProxyTCPFlow,
        proxy: ProviderSOCKSConfiguration,
        destination: SOCKS5Endpoint,
        unavailableFallback: UnavailableFallback,
        activityObserver: @escaping @Sendable (AppRoutingRelaySnapshot) -> Void,
        completion: @escaping @Sendable (UUID, TCPFlowRelayExit) -> Void
    ) {
        self.flow = flow
        self.proxy = proxy
        self.destination = destination
        failoverState = TCPRelayFailoverState(
            unavailableFallback: unavailableFallback
        )
        self.completion = completion
        let queue = DispatchQueue(label: "local.harry.myproxy.tcp-relay.\(id.uuidString)")
        self.queue = queue
        activityReporter = AppRoutingRelayActivityReporter(
            queue: queue,
            observer: activityObserver
        )
    }

    func start() {
        queue.async { [self] in
            guard !finished else { return }
            let connection = NWConnection(
                host: proxy.networkHost,
                port: proxy.networkPort,
                using: .tcp
            )
            self.connection = connection
            connection.stateUpdateHandler = { [weak self] state in
                guard let self else { return }
                self.handleConnectionState(state)
            }
            scheduleHandshakeTimeout()
            connection.start(queue: queue)
        }
    }

    func cancel() {
        queue.async { [self] in
            finish(error: TCPFlowRelayError.cancelled, allowDirectFallback: false)
        }
    }

    private func handleConnectionState(_ state: NWConnection.State) {
        guard !finished else { return }
        switch state {
        case .ready:
            guard !handshakeStarted else { return }
            handshakeStarted = true
            relayLocalPort = Self.localPort(of: connection)
            beginSOCKSHandshake()
        case let .failed(error):
            finish(error: TCPFlowRelayError.upstreamFailed(error.localizedDescription))
        case .cancelled:
            finish(error: nil)
        default:
            break
        }
    }

    private func scheduleHandshakeTimeout() {
        let work = DispatchWorkItem { [weak self] in
            self?.finish(error: TCPFlowRelayError.handshakeTimedOut)
        }
        handshakeTimeout = work
        queue.asyncAfter(deadline: .now() + .seconds(10), execute: work)
    }

    private func beginSOCKSHandshake() {
        do {
            let negotiator = SOCKS5ClientAuthenticationNegotiator(
                credentials: proxy.credentials
            )
            try send(negotiator.greeting()) { [weak self] in
                self?.receiveMethodSelection(negotiator: negotiator)
            }
        } catch {
            finish(error: error)
        }
    }

    private func receiveMethodSelection(
        negotiator: SOCKS5ClientAuthenticationNegotiator
    ) {
        receiveHandshakeChunk { [weak self] data in
            guard let self else { return }
            do {
                guard let selection = try methodDecoder.append(data) else {
                    receiveMethodSelection(negotiator: negotiator)
                    return
                }
                switch try negotiator.handle(selection) {
                case .authenticated:
                    sendConnectRequest()
                case let .sendUsernamePassword(request):
                    try send(request) { [weak self] in
                        self?.receiveAuthenticationResponse()
                    }
                }
            } catch {
                finish(error: error)
            }
        }
    }

    private func receiveAuthenticationResponse() {
        receiveHandshakeChunk { [weak self] data in
            guard let self else { return }
            do {
                guard let response = try authenticationDecoder.append(data) else {
                    receiveAuthenticationResponse()
                    return
                }
                try response.requireSuccess()
                sendConnectRequest()
            } catch {
                finish(error: error)
            }
        }
    }

    private func sendConnectRequest() {
        do {
            let request = try SOCKS5CommandRequest(
                command: .connect,
                endpoint: destination
            )
            try send(SOCKS5Codec.encodeCommandRequest(request)) { [weak self] in
                self?.receiveCommandResponse()
            }
        } catch {
            finish(error: error)
        }
    }

    private func receiveCommandResponse() {
        receiveHandshakeChunk { [weak self] data in
            guard let self else { return }
            do {
                guard let response = try commandDecoder.append(data) else {
                    receiveCommandResponse()
                    return
                }
                try response.requireSuccess()
                failoverState.markSOCKSHandshakeSucceeded()
                handshakeTimeout?.cancel()
                handshakeTimeout = nil
                openFlow(initialUpstreamPayload: commandDecoder.remainingData)
            } catch {
                finish(error: error)
            }
        }
    }

    private func send(_ data: Data, then next: @escaping @Sendable () -> Void) throws {
        guard let connection else {
            throw TCPFlowRelayError.upstreamClosedDuringHandshake
        }
        connection.send(content: data, completion: .contentProcessed { [weak self] error in
            guard let self else { return }
            if let error {
                self.finish(error: TCPFlowRelayError.upstreamFailed(
                    error.localizedDescription
                ))
            } else if !self.finished {
                next()
            }
        })
    }

    private func receiveHandshakeChunk(
        _ consume: @escaping @Sendable (Data) -> Void
    ) {
        guard let connection else {
            finish(error: TCPFlowRelayError.upstreamClosedDuringHandshake)
            return
        }
        connection.receive(
            minimumIncompleteLength: 1,
            maximumLength: 64 * 1_024
        ) { [weak self] data, _, isComplete, error in
            guard let self else { return }
            if let error {
                self.finish(error: TCPFlowRelayError.upstreamFailed(
                    error.localizedDescription
                ))
                return
            }
            if let data, !data.isEmpty {
                consume(data)
                return
            }
            if isComplete {
                self.finish(error: TCPFlowRelayError.upstreamClosedDuringHandshake)
            } else {
                self.receiveHandshakeChunk(consume)
            }
        }
    }

    private func openFlow(initialUpstreamPayload: Data) {
        let completion: @Sendable (Error?) -> Void = { [weak self] error in
            guard let self else { return }
            self.queue.async {
                if let error {
                    self.finish(error: TCPFlowRelayError.flowOpenFailed(
                        error.localizedDescription
                    ))
                    return
                }
                self.openedFlow = true
                self.report(.ready)
                if initialUpstreamPayload.isEmpty {
                    self.startPumps()
                } else {
                    self.byteLedger.recordUpstreamReceived(initialUpstreamPayload.count)
                    self.report(.relaying)
                    self.writeToFlow(initialUpstreamPayload) { [weak self] in
                        self?.byteLedger.recordAppDelivered(initialUpstreamPayload.count)
                        self?.report(.relaying)
                        self?.startPumps()
                    }
                }
            }
        }
        AppProxyFlowCompatibility.open(flow, completion: completion)
    }

    private func startPumps() {
        readFromFlow()
        readFromUpstream()
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
                    self.finish(error: TCPFlowRelayError.flowReadFailed(
                        error.localizedDescription
                    ))
                    return
                }
                guard let data, !data.isEmpty else {
                    self.halfCloseState.markAppReadEnded()
                    guard let connection = self.connection else {
                        self.finish(error: TCPFlowRelayError.upstreamFailed(
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
                                self.finish(error: TCPFlowRelayError.upstreamFailed(
                                    error.localizedDescription
                                ))
                            } else {
                                self.backpressureState.end(.appToUpstream)
                                self.finishIfBothHalvesClosed()
                            }
                        }
                    )
                    return
                }
                self.byteLedger.recordAppRead(data.count)
                guard let connection = self.connection else {
                    self.finish(error: TCPFlowRelayError.upstreamFailed(
                        "The connection disappeared before the payload was sent."
                    ))
                    return
                }
                connection.send(
                    content: data,
                    completion: .contentProcessed { [weak self] error in
                        guard let self else { return }
                        if let error {
                            self.finish(error: TCPFlowRelayError.upstreamFailed(
                                error.localizedDescription
                            ))
                        } else {
                            self.backpressureState.end(.appToUpstream)
                            self.byteLedger.recordUpstreamAccepted(data.count)
                            self.failoverState.markApplicationPayloadForwarded()
                            self.report(.relaying)
                            self.readFromFlow()
                        }
                    }
                )
            }
        }
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
                self.finish(error: TCPFlowRelayError.upstreamFailed(
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
                    self.finish(error: TCPFlowRelayError.flowWriteFailed(
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

    private func finish(
        error: Error?,
        allowDirectFallback: Bool = true
    ) {
        guard !finished else { return }
        if let error,
           allowDirectFallback,
           failoverState.canFallbackToDirect,
           !openedFlow {
            finishForDirectFallback(error: error)
            return
        }
        finished = true
        handshakeTimeout?.cancel()
        handshakeTimeout = nil
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
        completion(id, .finished)
    }

    private func finishForDirectFallback(error: Error) {
        finished = true
        handshakeTimeout?.cancel()
        handshakeTimeout = nil
        connection?.stateUpdateHandler = nil
        connection?.cancel()
        connection = nil
        completion(id, .directFallback(error.localizedDescription))
    }

    private func report(_ state: AppRoutingRelayState, error: String? = nil) {
        activityReporter.report(AppRoutingRelaySnapshot(
            state: state,
            uploadBytes: byteLedger.uploadBytes,
            downloadBytes: byteLedger.downloadBytes,
            error: error,
            localPort: relayLocalPort
        ))
    }

    private static func localPort(of connection: NWConnection?) -> UInt16? {
        guard case let .hostPort(_, port) = connection?.currentPath?.localEndpoint else {
            return nil
        }
        return port.rawValue
    }
}

final class TCPFlowRelayRegistry: @unchecked Sendable {
    private static let maximumActiveRelays = 512
    private let lock = NSLock()
    private var mihomoRelays: [UUID: TCPFlowRelay] = [:]
    private var directRelays: [UUID: DirectTCPFlowRelay] = [:]
    private var generation = UUID()

    func startMihomo(
        flow: NEAppProxyTCPFlow,
        proxy: ProviderSOCKSConfiguration,
        destination: SOCKS5Endpoint,
        directFallbackDestination: SOCKS5Endpoint? = nil,
        unavailableFallback: UnavailableFallback,
        activityObserver: @escaping @Sendable (AppRoutingRelaySnapshot) -> Void
    ) {
        let fallbackDestination = directFallbackDestination ?? destination
        let relayGeneration = currentGeneration()
        lock.lock()
        guard generation == relayGeneration else {
            lock.unlock()
            rejectBeforeStart(
                flow,
                error: .cancelled,
                activityObserver: activityObserver
            )
            return
        }
        guard mihomoRelays.count + directRelays.count < Self.maximumActiveRelays else {
            lock.unlock()
            rejectBeforeStart(
                flow,
                error: .capacityExceeded,
                activityObserver: activityObserver
            )
            return
        }
        let relay = TCPFlowRelay(
            flow: flow,
            proxy: proxy,
            destination: destination,
            unavailableFallback: unavailableFallback,
            activityObserver: activityObserver
        ) { [weak self] identifier, exit in
            self?.finishMihomo(
                identifier,
                exit: exit,
                generation: relayGeneration,
                flow: flow,
                destination: fallbackDestination,
                activityObserver: activityObserver
            )
        }
        mihomoRelays[relay.id] = relay
        lock.unlock()
        relay.start()
    }

    func startDirect(
        flow: NEAppProxyTCPFlow,
        destination: SOCKS5Endpoint,
        relayNote: String? = nil,
        activityObserver: @escaping @Sendable (AppRoutingRelaySnapshot) -> Void
    ) {
        startDirect(
            flow: flow,
            destination: destination,
            relayNote: relayNote,
            activityObserver: activityObserver,
            expectedGeneration: currentGeneration()
        )
    }

    func cancelAll() {
        lock.lock()
        generation = UUID()
        let currentMihomo = Array(mihomoRelays.values)
        let currentDirect = Array(directRelays.values)
        mihomoRelays.removeAll(keepingCapacity: false)
        directRelays.removeAll(keepingCapacity: false)
        lock.unlock()
        currentMihomo.forEach { $0.cancel() }
        currentDirect.forEach { $0.cancel() }
    }

    private func finishMihomo(
        _ identifier: UUID,
        exit: TCPFlowRelayExit,
        generation relayGeneration: UUID,
        flow: NEAppProxyTCPFlow,
        destination: SOCKS5Endpoint,
        activityObserver: @escaping @Sendable (AppRoutingRelaySnapshot) -> Void
    ) {
        switch exit {
        case .finished:
            lock.lock()
            mihomoRelays.removeValue(forKey: identifier)
            lock.unlock()
        case let .directFallback(reason):
            startDirect(
                flow: flow,
                destination: destination,
                relayNote: "Mihomo SOCKS setup failed before application payload forwarding; myproxy used the rule's Direct fallback. \(reason)",
                activityObserver: activityObserver,
                expectedGeneration: relayGeneration,
                replacingMihomo: identifier
            )
        }
    }

    private func startDirect(
        flow: NEAppProxyTCPFlow,
        destination: SOCKS5Endpoint,
        relayNote: String? = nil,
        activityObserver: @escaping @Sendable (AppRoutingRelaySnapshot) -> Void,
        expectedGeneration: UUID,
        replacingMihomo identifier: UUID? = nil
    ) {
        lock.lock()
        guard generation == expectedGeneration else {
            lock.unlock()
            rejectBeforeStart(
                flow,
                error: .cancelled,
                activityObserver: activityObserver
            )
            return
        }
        if let identifier {
            guard mihomoRelays.removeValue(forKey: identifier) != nil else {
                lock.unlock()
                rejectBeforeStart(
                    flow,
                    error: .cancelled,
                    activityObserver: activityObserver
                )
                return
            }
        } else {
            guard mihomoRelays.count + directRelays.count < Self.maximumActiveRelays else {
                lock.unlock()
                rejectBeforeStart(
                    flow,
                    error: .capacityExceeded,
                    activityObserver: activityObserver
                )
                return
            }
        }
        let relay = DirectTCPFlowRelay(
            flow: flow,
            destination: destination,
            relayNote: relayNote,
            activityObserver: activityObserver
        ) { [weak self] identifier in
            self?.removeDirect(identifier)
        }
        directRelays[relay.id] = relay
        lock.unlock()
        relay.start()
    }

    private func removeDirect(_ identifier: UUID) {
        lock.lock()
        directRelays.removeValue(forKey: identifier)
        lock.unlock()
    }

    private func currentGeneration() -> UUID {
        lock.lock()
        defer { lock.unlock() }
        return generation
    }

    private func rejectBeforeStart(
        _ flow: NEAppProxyTCPFlow,
        error: TCPFlowRelayError,
        activityObserver: @escaping @Sendable (AppRoutingRelaySnapshot) -> Void
    ) {
        flow.closeReadWithError(error)
        flow.closeWriteWithError(error)
        activityObserver(AppRoutingRelaySnapshot(
            state: .failed,
            uploadBytes: 0,
            downloadBytes: 0,
            error: error.localizedDescription,
            localPort: nil,
            effectiveAction: .reject
        ))
    }
}
