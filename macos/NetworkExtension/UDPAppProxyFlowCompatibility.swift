import Foundation
import MyproxyNetworkShared
@preconcurrency import Network
@preconcurrency import NetworkExtension

enum UDPFlowRelayError: Error, LocalizedError, Sendable {
    case controlConnectionClosed
    case invalidDatagramBatch(datagrams: Int, endpoints: Int)
    case unsupportedEndpoint
    case invalidRelayEndpoint
    case unexpectedControlData

    var errorDescription: String? {
        switch self {
        case .controlConnectionClosed:
            "The local mihomo SOCKS5 UDP control connection closed."
        case let .invalidDatagramBatch(datagrams, endpoints):
            "The intercepted UDP flow returned \(datagrams) datagrams and \(endpoints) endpoints."
        case .unsupportedEndpoint:
            "The UDP datagram endpoint is unsupported."
        case .invalidRelayEndpoint:
            "The SOCKS5 server returned an invalid UDP relay endpoint."
        case .unexpectedControlData:
            "The SOCKS5 UDP control connection returned unexpected payload data."
        }
    }
}

struct UDPFlowDatagram: Sendable {
    let payload: Data
    let endpoint: SOCKS5Endpoint
}

/// Bridges the macOS 14 `NWHostEndpoint` UDP flow API and the macOS 15
/// `Network.NWEndpoint` API without allowing deprecated endpoint wrappers to
/// escape the compatibility boundary.
enum UDPAppProxyFlowCompatibility {
    /// Drops only the datagrams whose endpoint cannot be converted and reports
    /// how many were skipped. An empty batch with `skipped > 0` is not EOF.
    static func readSkippingUnsupported(
        from flow: NEAppProxyUDPFlow,
        completion: @escaping @Sendable ([UDPFlowDatagram]?, Int, Error?) -> Void
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
        completion: @escaping @Sendable ([UDPFlowDatagram]?, Int, Error?) -> Void
    ) {
        flow.readDatagrams { values, error in
            guard let values else {
                completion(nil, 0, error)
                return
            }
            let converted = values.compactMap { payload, endpoint in
                (try? socksEndpoint(endpoint)).map {
                    UDPFlowDatagram(payload: payload, endpoint: $0)
                }
            }
            completion(converted, values.count - converted.count, error)
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
        completion: @escaping @Sendable ([UDPFlowDatagram]?, Int, Error?) -> Void
    ) {
        flow.__readDatagrams { datagrams, endpoints, error in
            guard let datagrams, let endpoints else {
                completion(nil, 0, error)
                return
            }
            guard datagrams.count == endpoints.count else {
                completion(
                    nil,
                    0,
                    UDPFlowRelayError.invalidDatagramBatch(
                        datagrams: datagrams.count,
                        endpoints: endpoints.count
                    )
                )
                return
            }
            let converted = zip(datagrams, endpoints).compactMap { payload, endpoint in
                (try? socksEndpoint(endpoint)).map {
                    UDPFlowDatagram(payload: payload, endpoint: $0)
                }
            }
            completion(converted, datagrams.count - converted.count, error)
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
