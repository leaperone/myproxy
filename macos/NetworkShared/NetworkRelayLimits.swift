import Foundation

/// Resource bounds shared by the Network Extension relay registries.
///
/// The default leaves enough room for the long-lived connections used by
/// browsers and editor helpers. A process launch environment may override the
/// relay count without rebuilding the extension, while the hard ceiling keeps
/// an accidental value from turning into an unbounded socket allocation.
public enum NetworkRelayLimits {
    public static let defaultMaximumActiveRelays = 4_096
    public static let maximumActiveRelays: Int = {
        guard let raw = ProcessInfo.processInfo.environment["MYPROXY_MAX_ACTIVE_RELAYS"],
              let value = Int(raw.trimmingCharacters(in: .whitespacesAndNewlines)),
              value > 0
        else {
            return defaultMaximumActiveRelays
        }
        return min(value, 16_384)
    }()

    /// Idle TCP streams are still allowed to be long-lived, but an abandoned
    /// stream must eventually release its Network Extension admission slot.
    public static let tcpIdleTimeoutSeconds: Int = 15 * 60
    public static let udpIdleTimeoutSeconds: Int = 120
    public static let tcpIdleTimeout: DispatchTimeInterval = .seconds(tcpIdleTimeoutSeconds)
    public static let udpIdleTimeout: DispatchTimeInterval = .seconds(udpIdleTimeoutSeconds)

    /// A TCP relay owns one intercepted flow and one NWConnection. The relay
    /// has at most one 64 KiB read/write chunk in each direction, so the
    /// payload buffer bound is about 128 KiB per TCP relay before object
    /// overhead. The budget includes a small allowance for control, logging,
    /// and DNS sockets. A launchd hard limit remains authoritative.
    public static let estimatedFileDescriptorsPerRelay = 2
    public static let estimatedRelayKinds = 2
    public static let reservedFileDescriptors = 256
}
