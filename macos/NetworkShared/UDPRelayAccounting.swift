import Foundation

/// Delivery-boundary accounting for a UDP relay.
///
/// Bytes read from an application or received from an upstream socket are not
/// user-visible traffic yet. Upload advances only after Network.framework
/// accepts the datagram send; download advances only after the intercepted app
/// flow accepts the corresponding write.
public struct UDPRelayByteLedger: Equatable, Sendable {
    public private(set) var applicationReadBytes: UInt64 = 0
    public private(set) var upstreamAcceptedBytes: UInt64 = 0
    public private(set) var upstreamReceivedBytes: UInt64 = 0
    public private(set) var applicationDeliveredBytes: UInt64 = 0

    public init() {}

    public mutating func recordApplicationRead(_ count: Int) {
        applicationReadBytes = Self.add(applicationReadBytes, count)
    }

    public mutating func recordUpstreamAccepted(_ count: Int) {
        upstreamAcceptedBytes = Self.add(upstreamAcceptedBytes, count)
    }

    public mutating func recordUpstreamReceived(_ count: Int) {
        upstreamReceivedBytes = Self.add(upstreamReceivedBytes, count)
    }

    public mutating func recordApplicationDelivered(_ count: Int) {
        applicationDeliveredBytes = Self.add(applicationDeliveredBytes, count)
    }

    public var uploadBytes: UInt64 { upstreamAcceptedBytes }
    public var downloadBytes: UInt64 { applicationDeliveredBytes }

    private static func add(_ current: UInt64, _ count: Int) -> UInt64 {
        guard count > 0 else { return current }
        let increment = UInt64(count)
        let (value, overflow) = current.addingReportingOverflow(increment)
        return overflow ? .max : value
    }
}

/// Bounds datagrams waiting to be written back into an intercepted app flow.
/// A slow or suspended consumer therefore cannot turn response bursts into an
/// unbounded Network Extension allocation.
public struct UDPRelayQueueBudget: Equatable, Sendable {
    public let maximumDatagrams: Int
    public let maximumBytes: Int
    public private(set) var datagramCount = 0
    public private(set) var byteCount = 0

    public init(maximumDatagrams: Int, maximumBytes: Int) {
        self.maximumDatagrams = max(1, maximumDatagrams)
        self.maximumBytes = max(1, maximumBytes)
    }

    public mutating func reserve(bytes: Int) -> Bool {
        guard canReserve(bytes: bytes) else { return false }
        datagramCount += 1
        byteCount += bytes
        return true
    }

    public func canReserve(bytes: Int) -> Bool {
        bytes >= 0
            && datagramCount < maximumDatagrams
            && bytes <= maximumBytes - byteCount
    }

    public mutating func release(bytes: Int) {
        guard datagramCount > 0 else { return }
        datagramCount -= 1
        byteCount = max(0, byteCount - max(0, bytes))
    }
}

/// UDP can safely switch from a failed SOCKS association to Direct only while
/// the app flow is still unopened and no application datagram was forwarded.
/// A successful UDP ASSOCIATE response alone does not consume app payload and
/// therefore does not close this fallback window.
public struct UDPRelayFailoverState: Equatable, Sendable {
    public let unavailableFallback: UnavailableFallback
    public private(set) var flowOpened = false
    public private(set) var applicationPayloadForwarded = false

    public init(unavailableFallback: UnavailableFallback) {
        self.unavailableFallback = unavailableFallback
    }

    public mutating func markFlowOpened() {
        flowOpened = true
    }

    public mutating func markApplicationPayloadForwarded() {
        applicationPayloadForwarded = true
    }

    public var canFallbackToDirect: Bool {
        unavailableFallback == .direct
            && !flowOpened
            && !applicationPayloadForwarded
    }
}
