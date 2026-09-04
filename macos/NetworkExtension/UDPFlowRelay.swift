import Foundation
import MyproxyNetworkShared
@preconcurrency import Network
@preconcurrency import NetworkExtension

enum UDPFlowRelayError: Error, LocalizedError, Sendable {
    case startupTimedOut
    case idleTimedOut
    case cancelled
    case controlConnectionClosed
    case controlConnectionFailed(String)
    case udpConnectionFailed(String)
    case flowOpenFailed(String)
    case flowReadFailed(String)
    case flowWriteFailed(String)
    case invalidDatagramBatch(datagrams: Int, endpoints: Int)
    case datagramTooLarge(limit: Int, actual: Int)
    case unexpectedDestination
    case unsupportedEndpoint
    case invalidRelayEndpoint
    case unexpectedControlData
    case incompleteRelayDatagram

    var errorDescription: String? {
        switch self {
        case .startupTimedOut:
            "The local mihomo SOCKS5 UDP association timed out."
        case .idleTimedOut:
            "The intercepted UDP flow exceeded its idle timeout."
        case .cancelled:
            "The intercepted UDP flow was cancelled."
        case .controlConnectionClosed:
            "The local mihomo SOCKS5 UDP control connection closed."
        case let .controlConnectionFailed(message):
            "The local mihomo SOCKS5 UDP control connection failed: \(message)"
        case let .udpConnectionFailed(message):
            "The local mihomo SOCKS5 UDP relay failed: \(message)"
        case let .flowOpenFailed(message):
            "The intercepted UDP flow could not be opened: \(message)"
        case let .flowReadFailed(message):
            "Reading the intercepted UDP flow failed: \(message)"
        case let .flowWriteFailed(message):
            "Writing the intercepted UDP flow failed: \(message)"
        case let .invalidDatagramBatch(datagrams, endpoints):
            "The intercepted UDP flow returned \(datagrams) datagrams and \(endpoints) endpoints."
        case let .datagramTooLarge(limit, actual):
            "The intercepted UDP datagram is \(actual) bytes; limit is \(limit)."
        case .unexpectedDestination:
            "The UDP flow changed destinations after its routing decision; myproxy stopped it rather than applying the wrong IP or port rule."
        case .unsupportedEndpoint:
            "The UDP datagram endpoint is unsupported."
        case .invalidRelayEndpoint:
            "The SOCKS5 server returned an invalid UDP relay endpoint."
        case .unexpectedControlData:
            "The SOCKS5 UDP control connection returned unexpected payload data."
        case .incompleteRelayDatagram:
            "The SOCKS5 UDP relay returned an incomplete datagram."
        }
    }
}

enum UDPFlowRelayExit: Sendable {
    case finished
    case directFallback(String)
}

struct UDPFlowDatagram: Sendable {
    let payload: Data
    let endpoint: SOCKS5Endpoint
}

/// Bridges the macOS 14 `NWHostEndpoint` UDP flow API and the macOS 15
/// `Network.NWEndpoint` API without allowing deprecated endpoint wrappers to
/// escape the compatibility boundary.
enum UDPAppProxyFlowCompatibility {
    static func read(
        from flow: NEAppProxyUDPFlow,
        completion: @escaping @Sendable ([UDPFlowDatagram]?, Error?) -> Void
    ) {
        if #available(macOS 15.0, *) {
            readModern(from: flow, completion: completion)
        } else {
            readLegacy(from: flow, completion: completion)
        }
    }

    static func write(
        _ datagram: UDPFlowDatagram,
        to flow: NEAppProxyUDPFlow,
        completion: @escaping @Sendable (Error?) -> Void
    ) {
        if #available(macOS 15.0, *) {
            writeModern(datagram, to: flow, completion: completion)
        } else {
            writeLegacy(datagram, to: flow, completion: completion)
        }
    }

    @available(macOS 15.0, *)
    private static func readModern(
        from flow: NEAppProxyUDPFlow,
        completion: @escaping @Sendable ([UDPFlowDatagram]?, Error?) -> Void
    ) {
        flow.readDatagrams { values, error in
            guard let values else {
                completion(nil, error)
                return
            }
            do {
                let converted = try values.map { payload, endpoint in
                    UDPFlowDatagram(
                        payload: payload,
                        endpoint: try socksEndpoint(endpoint)
                    )
                }
                completion(converted, error)
            } catch let conversionError {
                completion(nil, conversionError)
            }
        }
    }

    @available(macOS 15.0, *)
    private static func writeModern(
        _ datagram: UDPFlowDatagram,
        to flow: NEAppProxyUDPFlow,
        completion: @escaping @Sendable (Error?) -> Void
    ) {
        do {
            let endpoint = try modernEndpoint(datagram.endpoint)
            flow.writeDatagrams(
                [(datagram.payload, endpoint)],
                completionHandler: completion
            )
        } catch {
            completion(error)
        }
    }

    @available(macOS, introduced: 14.0, obsoleted: 15.0)
    private static func readLegacy(
        from flow: NEAppProxyUDPFlow,
        completion: @escaping @Sendable ([UDPFlowDatagram]?, Error?) -> Void
    ) {
        flow.__readDatagrams { datagrams, endpoints, error in
            guard let datagrams, let endpoints else {
                completion(nil, error)
                return
            }
            guard datagrams.count == endpoints.count else {
                completion(
                    nil,
                    UDPFlowRelayError.invalidDatagramBatch(
                        datagrams: datagrams.count,
                        endpoints: endpoints.count
                    )
                )
                return
            }
            do {
                let converted = try zip(datagrams, endpoints).map { payload, endpoint in
                    UDPFlowDatagram(
                        payload: payload,
                        endpoint: try socksEndpoint(endpoint)
                    )
                }
                completion(converted, error)
            } catch {
                completion(nil, error)
            }
        }
    }

    @available(macOS, introduced: 14.0, obsoleted: 15.0)
    private static func writeLegacy(
        _ datagram: UDPFlowDatagram,
        to flow: NEAppProxyUDPFlow,
        completion: @escaping @Sendable (Error?) -> Void
    ) {
        do {
            let endpoint = try legacyEndpoint(datagram.endpoint)
            flow.__writeDatagrams(
                [datagram.payload],
                sentBy: [endpoint],
                completionHandler: completion
            )
        } catch {
            completion(error)
        }
    }

    @available(macOS 15.0, *)
    private static func socksEndpoint(_ endpoint: Network.NWEndpoint) throws -> SOCKS5Endpoint {
        guard case let .hostPort(host, port) = endpoint else {
            throw UDPFlowRelayError.unsupportedEndpoint
        }
        return try socksEndpoint(host: host.debugDescription, port: port.rawValue)
    }

    @available(macOS, introduced: 14.0, obsoleted: 15.0)
    private static func socksEndpoint(
        _ endpoint: NetworkExtension.__NWEndpoint
    ) throws -> SOCKS5Endpoint {
        let object = endpoint as NSObject
        guard object.isKind(of: NetworkExtension.__NWHostEndpoint.self),
              let host = object.value(forKey: "hostname") as? String,
              let portString = object.value(forKey: "port") as? String,
              let port = UInt16(portString)
        else {
            throw UDPFlowRelayError.unsupportedEndpoint
        }
        return try socksEndpoint(host: host, port: port)
    }

    private static func socksEndpoint(host: String, port: UInt16) throws -> SOCKS5Endpoint {
        guard port > 0 else { throw UDPFlowRelayError.unsupportedEndpoint }
        var normalizedHost = host.trimmingCharacters(in: .whitespacesAndNewlines)
        if normalizedHost.hasPrefix("["), normalizedHost.hasSuffix("]") {
            normalizedHost.removeFirst()
            normalizedHost.removeLast()
        }
        if let address = try? IPAddress(normalizedHost) {
            return SOCKS5Endpoint(
                address: SOCKS5Address(ipAddress: address),
                port: port
            )
        }
        return SOCKS5Endpoint(
            address: try SOCKS5Address(domain: normalizedHost),
            port: port
        )
    }

    @available(macOS 15.0, *)
    private static func modernEndpoint(_ endpoint: SOCKS5Endpoint) throws -> Network.NWEndpoint {
        guard endpoint.port > 0,
              let port = NWEndpoint.Port(rawValue: endpoint.port)
        else {
            throw UDPFlowRelayError.unsupportedEndpoint
        }
        return .hostPort(
            host: NWEndpoint.Host(try hostString(endpoint.address)),
            port: port
        )
    }

    @available(macOS, introduced: 14.0, obsoleted: 15.0)
    private static func legacyEndpoint(
        _ endpoint: SOCKS5Endpoint
    ) throws -> NetworkExtension.__NWEndpoint {
        guard endpoint.port > 0,
              let endpointClass = NSClassFromString("NWHostEndpoint") as? NSObject.Type
        else {
            throw UDPFlowRelayError.unsupportedEndpoint
        }
        let selector = NSSelectorFromString("endpointWithHostname:port:")
        guard let value = endpointClass.perform(
            selector,
            with: try hostString(endpoint.address),
            with: String(endpoint.port)
        )?.takeUnretainedValue() as? NetworkExtension.__NWEndpoint
        else {
            throw UDPFlowRelayError.unsupportedEndpoint
        }
        return value
    }

    private static func hostString(_ address: SOCKS5Address) throws -> String {
        if let ipAddress = address.ipAddress { return ipAddress.presentation }
        if let domain = address.domain { return domain }
        throw UDPFlowRelayError.unsupportedEndpoint
    }
}

/// Owns a SOCKS5 UDP ASSOCIATE control connection and its corresponding UDP
/// relay socket. At most one application read and one relay read are in flight;
/// the next read starts only after the preceding send/write completion.
final class UDPFlowRelay: @unchecked Sendable {
    let id = UUID()

    private enum Limits {
        static let startupTimeout: DispatchTimeInterval = .seconds(10)
        static let idleTimeout: DispatchTimeInterval = .seconds(120)
    }

    private let flow: NEAppProxyUDPFlow
    private let proxy: ProviderSOCKSConfiguration
    private let expectedDestination: SOCKS5Endpoint
    private let queue: DispatchQueue
    private let completion: @Sendable (UUID, UDPFlowRelayExit) -> Void
    private let activityReporter: AppRoutingRelayActivityReporter

    private var controlConnection: NWConnection?
    private var udpConnection: NWConnection?
    private var startupTimeout: DispatchWorkItem?
    private var idleTimeout: DispatchWorkItem?
    private var idleGeneration: UInt64 = 0
    private var methodDecoder = SOCKS5MethodSelectionDecoder()
    private var authenticationDecoder = SOCKS5UsernamePasswordResponseDecoder()
    private var commandDecoder = SOCKS5CommandReplyDecoder()
    private var startedHandshake = false
    private var startedUDPConnection = false
    private var openedFlow = false
    private var failoverState: UDPRelayFailoverState
    private var finished = false
    private var byteLedger = UDPRelayByteLedger()
    private var relayLocalPort: UInt16?

    init(
        flow: NEAppProxyUDPFlow,
        proxy: ProviderSOCKSConfiguration,
        expectedDestination: SOCKS5Endpoint,
        unavailableFallback: UnavailableFallback,
        activityObserver: @escaping @Sendable (AppRoutingRelaySnapshot) -> Void,
        completion: @escaping @Sendable (UUID, UDPFlowRelayExit) -> Void
    ) {
        self.flow = flow
        self.proxy = proxy
        self.expectedDestination = expectedDestination
        failoverState = UDPRelayFailoverState(
            unavailableFallback: unavailableFallback
        )
        self.completion = completion
        let queue = DispatchQueue(label: "local.harry.myproxy.udp-relay.\(id.uuidString)")
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
            controlConnection = connection
            connection.stateUpdateHandler = { [weak self] state in
                guard let self else { return }
                self.queue.async { self.handleControlState(state) }
            }
            scheduleStartupTimeout()
            connection.start(queue: queue)
        }
    }

    func cancel() {
        queue.async { [self] in
            finish(error: UDPFlowRelayError.cancelled, allowDirectFallback: false)
        }
    }

    private func handleControlState(_ state: NWConnection.State) {
        guard !finished else { return }
        switch state {
        case .ready where !startedHandshake:
            startedHandshake = true
            beginSOCKSHandshake()
        case let .failed(error):
            finish(error: UDPFlowRelayError.controlConnectionFailed(
                error.localizedDescription
            ))
        case .cancelled:
            finish(error: UDPFlowRelayError.controlConnectionClosed)
        default:
            break
        }
    }

    private func scheduleStartupTimeout() {
        let work = DispatchWorkItem { [weak self] in
            self?.finish(error: UDPFlowRelayError.startupTimedOut)
        }
        startupTimeout = work
        queue.asyncAfter(deadline: .now() + Limits.startupTimeout, execute: work)
    }

    private func beginSOCKSHandshake() {
        do {
            let negotiator = SOCKS5ClientAuthenticationNegotiator(
                credentials: proxy.credentials
            )
            try sendControl(negotiator.greeting()) { [weak self] in
                self?.receiveMethodSelection(negotiator: negotiator)
            }
        } catch {
            finish(error: error)
        }
    }

    private func receiveMethodSelection(
        negotiator: SOCKS5ClientAuthenticationNegotiator
    ) {
        receiveControlHandshakeChunk { [weak self] data in
            guard let self else { return }
            do {
                guard let selection = try methodDecoder.append(data) else {
                    receiveMethodSelection(negotiator: negotiator)
                    return
                }
                switch try negotiator.handle(selection) {
                case .authenticated:
                    sendUDPAssociateRequest()
                case let .sendUsernamePassword(request):
                    try sendControl(request) { [weak self] in
                        self?.receiveAuthenticationResponse()
                    }
                }
            } catch {
                finish(error: error)
            }
        }
    }

    private func receiveAuthenticationResponse() {
        receiveControlHandshakeChunk { [weak self] data in
            guard let self else { return }
            do {
                guard let response = try authenticationDecoder.append(data) else {
                    receiveAuthenticationResponse()
                    return
                }
                try response.requireSuccess()
                sendUDPAssociateRequest()
            } catch {
                finish(error: error)
            }
        }
    }

    private func sendUDPAssociateRequest() {
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
                self?.receiveUDPAssociateResponse()
            }
        } catch {
            finish(error: error)
        }
    }

    private func receiveUDPAssociateResponse() {
        receiveControlHandshakeChunk { [weak self] data in
            guard let self else { return }
            do {
                guard let response = try commandDecoder.append(data) else {
                    receiveUDPAssociateResponse()
                    return
                }
                let boundEndpoint = try response.requireSuccess()
                guard commandDecoder.remainingData.isEmpty else {
                    throw UDPFlowRelayError.unexpectedControlData
                }
                let relayEndpoint = try normalizedRelayEndpoint(boundEndpoint)
                monitorControlConnection()
                startUDPConnection(to: relayEndpoint)
            } catch {
                finish(error: error)
            }
        }
    }

    private func normalizedRelayEndpoint(
        _ endpoint: SOCKS5Endpoint
    ) throws -> SOCKS5Endpoint {
        guard endpoint.port > 0 else {
            throw UDPFlowRelayError.invalidRelayEndpoint
        }
        if endpoint.address.ipAddress?.isUnspecified == true {
            let address: SOCKS5Address
            if let ipAddress = try? IPAddress(proxy.host) {
                address = SOCKS5Address(ipAddress: ipAddress)
            } else {
                address = try SOCKS5Address(domain: proxy.host)
            }
            return SOCKS5Endpoint(address: address, port: endpoint.port)
        }
        return endpoint
    }

    private func startUDPConnection(to endpoint: SOCKS5Endpoint) {
        do {
            let host: String
            if let ipAddress = endpoint.address.ipAddress {
                host = ipAddress.presentation
            } else if let domain = endpoint.address.domain {
                host = domain
            } else {
                throw UDPFlowRelayError.invalidRelayEndpoint
            }
            guard let port = NWEndpoint.Port(rawValue: endpoint.port) else {
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
                self.queue.async { self.handleUDPState(state) }
            }
            connection.start(queue: queue)
        } catch {
            finish(error: error)
        }
    }

    private func handleUDPState(_ state: NWConnection.State) {
        guard !finished else { return }
        switch state {
        case .ready where !startedUDPConnection:
            startedUDPConnection = true
            relayLocalPort = Self.localPort(of: udpConnection)
            openFlow()
        case let .failed(error):
            finish(error: UDPFlowRelayError.udpConnectionFailed(
                error.localizedDescription
            ))
        case .cancelled:
            finish(error: UDPFlowRelayError.udpConnectionFailed(
                "The UDP relay connection was cancelled."
            ))
        default:
            break
        }
    }

    private func sendControl(
        _ data: Data,
        then next: @escaping @Sendable () -> Void
    ) throws {
        guard let controlConnection else {
            throw UDPFlowRelayError.controlConnectionClosed
        }
        controlConnection.send(
            content: data,
            completion: .contentProcessed { [weak self] error in
                guard let self else { return }
                self.queue.async {
                    if let error {
                        self.finish(error: UDPFlowRelayError.controlConnectionFailed(
                            error.localizedDescription
                        ))
                    } else if !self.finished {
                        next()
                    }
                }
            }
        )
    }

    private func receiveControlHandshakeChunk(
        _ consume: @escaping @Sendable (Data) -> Void
    ) {
        guard let controlConnection else {
            finish(error: UDPFlowRelayError.controlConnectionClosed)
            return
        }
        controlConnection.receive(
            minimumIncompleteLength: 1,
            maximumLength: 64 * 1_024
        ) { [weak self] data, _, isComplete, error in
            guard let self else { return }
            self.queue.async {
                if let error {
                    self.finish(error: UDPFlowRelayError.controlConnectionFailed(
                        error.localizedDescription
                    ))
                } else if let data, !data.isEmpty {
                    consume(data)
                } else if isComplete {
                    self.finish(error: UDPFlowRelayError.controlConnectionClosed)
                } else {
                    self.receiveControlHandshakeChunk(consume)
                }
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
            self.queue.async {
                if let error {
                    self.finish(error: UDPFlowRelayError.controlConnectionFailed(
                        error.localizedDescription
                    ))
                } else if let data, !data.isEmpty {
                    self.finish(error: UDPFlowRelayError.unexpectedControlData)
                } else if isComplete {
                    self.finish(error: UDPFlowRelayError.controlConnectionClosed)
                } else {
                    self.monitorControlConnection()
                }
            }
        }
    }

    private func openFlow() {
        AppProxyFlowCompatibility.open(flow) { [weak self] error in
            guard let self else { return }
            self.queue.async {
                if let error {
                    self.finish(error: UDPFlowRelayError.flowOpenFailed(
                        error.localizedDescription
                    ))
                    return
                }
                self.openedFlow = true
                self.failoverState.markFlowOpened()
                self.report(.ready)
                self.startupTimeout?.cancel()
                self.startupTimeout = nil
                self.resetIdleTimeout()
                self.readFromFlow()
                self.readFromRelay()
            }
        }
    }

    private func resetIdleTimeout() {
        idleTimeout?.cancel()
        idleGeneration &+= 1
        let generation = idleGeneration
        let work = DispatchWorkItem { [weak self] in
            guard let self, self.idleGeneration == generation else { return }
            self.finish(error: UDPFlowRelayError.idleTimedOut)
        }
        idleTimeout = work
        queue.asyncAfter(deadline: .now() + Limits.idleTimeout, execute: work)
    }

    private func readFromFlow() {
        guard !finished else { return }
        UDPAppProxyFlowCompatibility.read(from: flow) { [weak self] datagrams, error in
            guard let self else { return }
            self.queue.async {
                if let error {
                    self.finish(error: UDPFlowRelayError.flowReadFailed(
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
                    self.sendToRelay(datagrams, index: 0)
                } catch {
                    self.finish(error: error)
                }
            }
        }
    }

    private func validate(_ datagrams: [UDPFlowDatagram]) throws {
        for datagram in datagrams {
            guard datagram.endpoint == expectedDestination else {
                throw UDPFlowRelayError.unexpectedDestination
            }
            guard datagram.payload.count <= SOCKS5Limits.maximumUDPDatagramBytes else {
                throw UDPFlowRelayError.datagramTooLarge(
                    limit: SOCKS5Limits.maximumUDPDatagramBytes,
                    actual: datagram.payload.count
                )
            }
        }
    }

    private func sendToRelay(_ datagrams: [UDPFlowDatagram], index: Int) {
        guard !finished else { return }
        guard index < datagrams.count else {
            readFromFlow()
            return
        }
        guard let udpConnection else {
            finish(error: UDPFlowRelayError.udpConnectionFailed(
                "The UDP relay connection is unavailable."
            ))
            return
        }
        do {
            let item = datagrams[index]
            let wireData = try SOCKS5Codec.encodeUDPDatagram(
                SOCKS5UDPDatagram(
                    destination: item.endpoint,
                    payload: item.payload
                )
            )
            udpConnection.send(
                content: wireData,
                contentContext: .defaultMessage,
                isComplete: true,
                completion: .contentProcessed { [weak self] error in
                    guard let self else { return }
                    self.queue.async {
                        if let error {
                            self.finish(error: UDPFlowRelayError.udpConnectionFailed(
                                error.localizedDescription
                            ))
                        } else {
                            self.byteLedger.recordUpstreamAccepted(item.payload.count)
                            self.failoverState.markApplicationPayloadForwarded()
                            self.report(.relaying)
                            self.sendToRelay(datagrams, index: index + 1)
                        }
                    }
                }
            )
        } catch {
            finish(error: error)
        }
    }

    private func readFromRelay() {
        guard !finished, let udpConnection else { return }
        udpConnection.receiveMessage { [weak self] data, _, isComplete, error in
            guard let self else { return }
            self.queue.async {
                if let error {
                    self.finish(error: UDPFlowRelayError.udpConnectionFailed(
                        error.localizedDescription
                    ))
                    return
                }
                guard isComplete else {
                    self.finish(error: UDPFlowRelayError.incompleteRelayDatagram)
                    return
                }
                guard let data else {
                    self.readFromRelay()
                    return
                }
                do {
                    let decoded = try SOCKS5Codec.decodeUDPDatagram(data)
                    let datagram = UDPFlowDatagram(
                        payload: decoded.payload,
                        endpoint: decoded.destination
                    )
                    self.byteLedger.recordUpstreamReceived(decoded.payload.count)
                    self.resetIdleTimeout()
                    UDPAppProxyFlowCompatibility.write(
                        datagram,
                        to: self.flow
                    ) { [weak self] error in
                        guard let self else { return }
                        self.queue.async {
                            if let error {
                                self.finish(error: UDPFlowRelayError.flowWriteFailed(
                                    error.localizedDescription
                                ))
                            } else {
                                self.byteLedger.recordApplicationDelivered(
                                    decoded.payload.count
                                )
                                self.report(.relaying)
                                self.readFromRelay()
                            }
                        }
                    }
                } catch {
                    self.finish(error: error)
                }
            }
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
        startupTimeout?.cancel()
        startupTimeout = nil
        idleTimeout?.cancel()
        idleTimeout = nil

        controlConnection?.stateUpdateHandler = nil
        controlConnection?.cancel()
        controlConnection = nil
        udpConnection?.stateUpdateHandler = nil
        udpConnection?.cancel()
        udpConnection = nil

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
        completion(id, .finished)
    }

    private func finishForDirectFallback(error: Error) {
        finished = true
        startupTimeout?.cancel()
        startupTimeout = nil
        idleTimeout?.cancel()
        idleTimeout = nil
        controlConnection?.stateUpdateHandler = nil
        controlConnection?.cancel()
        controlConnection = nil
        udpConnection?.stateUpdateHandler = nil
        udpConnection?.cancel()
        udpConnection = nil
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

final class UDPFlowRelayRegistry: @unchecked Sendable {
    private static let maximumActiveRelays = 512
    private let lock = NSLock()
    private var mihomoRelays: [UUID: UDPFlowRelay] = [:]
    private var directRelays: [UUID: DirectUDPFlowRelay] = [:]
    private var generation = UUID()

    func startMihomo(
        flow: NEAppProxyUDPFlow,
        proxy: ProviderSOCKSConfiguration,
        expectedDestination: SOCKS5Endpoint,
        unavailableFallback: UnavailableFallback,
        activityObserver: @escaping @Sendable (AppRoutingRelaySnapshot) -> Void
    ) {
        let relayGeneration = currentGeneration()
        let relay = UDPFlowRelay(
            flow: flow,
            proxy: proxy,
            expectedDestination: expectedDestination,
            unavailableFallback: unavailableFallback,
            activityObserver: activityObserver
        ) { [weak self] identifier, exit in
            self?.finishMihomo(
                identifier,
                exit: exit,
                generation: relayGeneration,
                flow: flow,
                expectedDestination: expectedDestination,
                activityObserver: activityObserver
            )
        }
        lock.lock()
        guard generation == relayGeneration,
              mihomoRelays.count + directRelays.count < Self.maximumActiveRelays else {
            lock.unlock()
            relay.cancel()
            return
        }
        mihomoRelays[relay.id] = relay
        lock.unlock()
        relay.start()
    }

    func startDirect(
        flow: NEAppProxyUDPFlow,
        expectedDestination: SOCKS5Endpoint,
        relayNote: String? = nil,
        activityObserver: @escaping @Sendable (AppRoutingRelaySnapshot) -> Void
    ) {
        startDirect(
            flow: flow,
            expectedDestination: expectedDestination,
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
        exit: UDPFlowRelayExit,
        generation relayGeneration: UUID,
        flow: NEAppProxyUDPFlow,
        expectedDestination: SOCKS5Endpoint,
        activityObserver: @escaping @Sendable (AppRoutingRelaySnapshot) -> Void
    ) {
        lock.lock()
        let wasRegistered = mihomoRelays.removeValue(forKey: identifier) != nil
        let mayTransition = wasRegistered && generation == relayGeneration
        lock.unlock()

        guard mayTransition,
              case let .directFallback(reason) = exit else { return }
        startDirect(
            flow: flow,
            expectedDestination: expectedDestination,
            relayNote: "Mihomo SOCKS UDP setup failed before application payload forwarding; myproxy used the rule's Direct fallback. \(reason)",
            activityObserver: activityObserver,
            expectedGeneration: relayGeneration
        )
    }

    private func startDirect(
        flow: NEAppProxyUDPFlow,
        expectedDestination: SOCKS5Endpoint,
        relayNote: String?,
        activityObserver: @escaping @Sendable (AppRoutingRelaySnapshot) -> Void,
        expectedGeneration: UUID
    ) {
        let relay = DirectUDPFlowRelay(
            flow: flow,
            expectedDestination: expectedDestination,
            relayNote: relayNote,
            activityObserver: activityObserver
        ) { [weak self] identifier in
            self?.removeDirect(identifier)
        }
        lock.lock()
        guard generation == expectedGeneration,
              mihomoRelays.count + directRelays.count < Self.maximumActiveRelays else {
            lock.unlock()
            relay.cancel()
            return
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
}
