import Darwin
import Foundation
import MyproxyNetworkShared

/// Raises the extension's soft descriptor limit before accepting flows.
/// For the default 4096 relays this asks for 18,688 descriptors. That covers
/// two descriptors per relay across TCP and UDP, two descriptors for each of
/// the 1,024 UDP conversations, and 256 control descriptors. Failure is
/// observable in the extension log; admission shrinks to the usable limit.
enum RelayResourceLimits {
    private final class State: @unchecked Sendable {
        let lock = NSLock()
        var effectiveMaximumActiveRelays = NetworkRelayLimits.maximumActiveRelays
    }

    private static let state = State()

    static var maximumActiveRelays: Int {
        state.lock.lock()
        defer { state.lock.unlock() }
        return state.effectiveMaximumActiveRelays
    }

    static func prepare() {
        var current = rlimit()
        guard getrlimit(RLIMIT_NOFILE, &current) == 0 else {
            AppLog.warn("NetworkExtension.Resources", "getrlimit(RLIMIT_NOFILE) failed errno=\(errno)")
            return
        }

        let requested = NetworkRelayLimits.maximumActiveRelays
            * NetworkRelayLimits.estimatedFileDescriptorsPerRelay
            * NetworkRelayLimits.estimatedRelayKinds
            + NetworkRelayLimits.maximumActiveUDPConversations
                * NetworkRelayLimits.estimatedFileDescriptorsPerUDPConversation
            + NetworkRelayLimits.reservedFileDescriptors
        let target = min(rlim_t(requested), current.rlim_max)
        let hardLimitEffective = maximumRelaysAllowedByFileLimit(current.rlim_max)
        guard current.rlim_cur < target else {
            setEffectiveMaximumActiveRelays(hardLimitEffective)
            AppLog.info(
                "NetworkExtension.Resources",
                "relay_capacity=\(hardLimitEffective) configured=\(NetworkRelayLimits.maximumActiveRelays) nofile_soft=\(current.rlim_cur) nofile_hard=\(current.rlim_max)"
            )
            return
        }

        var raised = current
        raised.rlim_cur = target
        guard setrlimit(RLIMIT_NOFILE, &raised) == 0 else {
            let softLimitEffective = maximumRelaysAllowedByFileLimit(current.rlim_cur)
            setEffectiveMaximumActiveRelays(softLimitEffective)
            AppLog.warn(
                "NetworkExtension.Resources",
                "setrlimit(RLIMIT_NOFILE) failed requested=\(target) relay_capacity=\(softLimitEffective) current=\(current.rlim_cur) hard=\(current.rlim_max) errno=\(errno)"
            )
            return
        }
        setEffectiveMaximumActiveRelays(hardLimitEffective)
        AppLog.info(
            "NetworkExtension.Resources",
            "relay_capacity=\(hardLimitEffective) configured=\(NetworkRelayLimits.maximumActiveRelays) nofile_soft=\(target) nofile_hard=\(current.rlim_max)"
        )
    }

    private static func setEffectiveMaximumActiveRelays(_ value: Int) {
        state.lock.lock()
        state.effectiveMaximumActiveRelays = value
        state.lock.unlock()
    }

    private static func maximumRelaysAllowedByFileLimit(_ fileLimit: rlim_t) -> Int {
        // Darwin exposes RLIM_INFINITY as an unavailable C macro in Swift.
        guard fileLimit != (rlim_t.max >> 1) else {
            return NetworkRelayLimits.maximumActiveRelays
        }
        let perRelay = rlim_t(
            NetworkRelayLimits.estimatedFileDescriptorsPerRelay
                * NetworkRelayLimits.estimatedRelayKinds
        )
        let reserved = rlim_t(
            NetworkRelayLimits.reservedFileDescriptors
                + NetworkRelayLimits.maximumActiveUDPConversations
                    * NetworkRelayLimits.estimatedFileDescriptorsPerUDPConversation
        )
        guard fileLimit > reserved, perRelay > 0 else { return 1 }
        let available = (fileLimit - reserved) / perRelay
        return min(
            NetworkRelayLimits.maximumActiveRelays,
            max(1, Int(min(available, rlim_t(Int.max))))
        )
    }
}
