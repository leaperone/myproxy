import Foundation

/// The four byte boundaries of an intercepted TCP relay.
///
/// Keeping the boundaries separate prevents UI telemetry from claiming bytes
/// merely because they were read from one side of the relay. `uploadBytes` is
/// advanced only after Network.framework accepts an upstream send, while
/// `downloadBytes` advances only after the intercepted application flow
/// accepts a write.
public struct TCPRelayByteLedger: Codable, Equatable, Sendable {
    public private(set) var appRead: UInt64
    public private(set) var upstreamAccepted: UInt64
    public private(set) var upstreamReceived: UInt64
    public private(set) var appDelivered: UInt64

    public init(
        appRead: UInt64 = 0,
        upstreamAccepted: UInt64 = 0,
        upstreamReceived: UInt64 = 0,
        appDelivered: UInt64 = 0
    ) {
        self.appRead = appRead
        self.upstreamAccepted = upstreamAccepted
        self.upstreamReceived = upstreamReceived
        self.appDelivered = appDelivered
    }

    /// Bytes safe to present as uploaded by the relay.
    public var uploadBytes: UInt64 { upstreamAccepted }

    /// Bytes safe to present as downloaded by the application.
    public var downloadBytes: UInt64 { appDelivered }

    public mutating func recordAppRead(_ byteCount: Int) {
        appRead = Self.saturatingAdd(appRead, byteCount: byteCount)
    }

    public mutating func recordUpstreamAccepted(_ byteCount: Int) {
        upstreamAccepted = Self.saturatingAdd(upstreamAccepted, byteCount: byteCount)
    }

    public mutating func recordUpstreamReceived(_ byteCount: Int) {
        upstreamReceived = Self.saturatingAdd(upstreamReceived, byteCount: byteCount)
    }

    public mutating func recordAppDelivered(_ byteCount: Int) {
        appDelivered = Self.saturatingAdd(appDelivered, byteCount: byteCount)
    }

    private static func saturatingAdd(_ value: UInt64, byteCount: Int) -> UInt64 {
        guard byteCount > 0 else { return value }
        let increment = UInt64(byteCount)
        let (sum, overflow) = value.addingReportingOverflow(increment)
        return overflow ? .max : sum
    }
}

/// Small, transport-independent half-close state machine shared by TCP relay
/// implementations and their tests. TCP completes normally only after the
/// application has ended its upload half and the upstream has ended its
/// download half; either half may close first.
public struct TCPRelayHalfCloseState: Equatable, Sendable {
    public private(set) var appReadEnded = false
    public private(set) var upstreamReadEnded = false

    public init() {}

    public var bothReadHalvesEnded: Bool {
        appReadEnded && upstreamReadEnded
    }

    public mutating func markAppReadEnded() {
        appReadEnded = true
    }

    public mutating func markUpstreamReadEnded() {
        upstreamReadEnded = true
    }
}

/// Decides whether a failed Mihomo SOCKS setup can still be replaced by a
/// direct connection without duplicating or losing application payload.
public struct TCPRelayFailoverState: Equatable, Sendable {
    public let unavailableFallback: UnavailableFallback
    public private(set) var socksHandshakeSucceeded = false
    public private(set) var applicationPayloadForwarded = false

    public init(unavailableFallback: UnavailableFallback) {
        self.unavailableFallback = unavailableFallback
    }

    public var canFallbackToDirect: Bool {
        unavailableFallback == .direct
            && !socksHandshakeSucceeded
            && !applicationPayloadForwarded
    }

    public mutating func markSOCKSHandshakeSucceeded() {
        socksHandshakeSucceeded = true
    }

    public mutating func markApplicationPayloadForwarded() {
        applicationPayloadForwarded = true
    }
}

public enum TCPRelayDirection: Sendable {
    case appToUpstream
    case upstreamToApp
}

/// Explicitly enforces at most one in-flight read/write chain per direction.
/// The two directions remain independent so a slow upload does not stall a
/// download, while each direction retains a strict one-chunk memory bound.
public struct TCPRelayBackpressureState: Equatable, Sendable {
    public private(set) var appToUpstreamInFlight = false
    public private(set) var upstreamToAppInFlight = false

    public init() {}

    @discardableResult
    public mutating func begin(_ direction: TCPRelayDirection) -> Bool {
        switch direction {
        case .appToUpstream:
            guard !appToUpstreamInFlight else { return false }
            appToUpstreamInFlight = true
        case .upstreamToApp:
            guard !upstreamToAppInFlight else { return false }
            upstreamToAppInFlight = true
        }
        return true
    }

    public mutating func end(_ direction: TCPRelayDirection) {
        switch direction {
        case .appToUpstream:
            appToUpstreamInFlight = false
        case .upstreamToApp:
            upstreamToAppInFlight = false
        }
    }
}
