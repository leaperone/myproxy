import Darwin
import Foundation
import MyproxyNetworkShared

/// Raises the extension's soft descriptor limit before accepting flows.
/// For the default 4096 relays this asks for 8448 descriptors, covering two
/// descriptors per relay plus 256 control descriptors. Failure is observable
/// in the extension log; the relay registry still keeps its bounded admission
/// guard when the launchd hard limit is lower.
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
            + NetworkRelayLimits.reservedFileDescriptors
        let target = min(rlim_t(requested), current.rlim_max)
        let effective = maximumRelaysAllowedByHardLimit(current.rlim_max)
        state.lock.lock()
        state.effectiveMaximumActiveRelays = effective
        state.lock.unlock()
        guard current.rlim_cur < target else {
            AppLog.info(
                "NetworkExtension.Resources",
                "relay_capacity=\(effective) configured=\(NetworkRelayLimits.maximumActiveRelays) nofile_soft=\(current.rlim_cur) nofile_hard=\(current.rlim_max)"
            )
            return
        }

        var raised = current
        raised.rlim_cur = target
        guard setrlimit(RLIMIT_NOFILE, &raised) == 0 else {
            AppLog.warn(
                "NetworkExtension.Resources",
                "setrlimit(RLIMIT_NOFILE) failed requested=\(target) relay_capacity=\(effective) current=\(current.rlim_cur) hard=\(current.rlim_max) errno=\(errno)"
            )
            return
        }
        AppLog.info(
            "NetworkExtension.Resources",
            "relay_capacity=\(effective) configured=\(NetworkRelayLimits.maximumActiveRelays) nofile_soft=\(target) nofile_hard=\(current.rlim_max)"
        )
    }

    private static func maximumRelaysAllowedByHardLimit(_ hardLimit: rlim_t) -> Int {
        // Darwin exposes RLIM_INFINITY as an unavailable C macro in Swift.
        guard hardLimit != (rlim_t.max >> 1) else {
            return NetworkRelayLimits.maximumActiveRelays
        }
        let perRelay = rlim_t(
            NetworkRelayLimits.estimatedFileDescriptorsPerRelay
                * NetworkRelayLimits.estimatedRelayKinds
        )
        let reserved = rlim_t(NetworkRelayLimits.reservedFileDescriptors)
        guard hardLimit > reserved, perRelay > 0 else { return 1 }
        let available = (hardLimit - reserved) / perRelay
        return min(
            NetworkRelayLimits.maximumActiveRelays,
            max(1, Int(min(available, rlim_t(Int.max))))
        )
    }
}
