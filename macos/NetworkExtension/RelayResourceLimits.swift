import Darwin
import Foundation
import MyproxyNetworkShared

/// Raises the extension's soft descriptor limit before accepting flows.
/// For the default 4096 relays this asks for 8448 descriptors, covering two
/// descriptors per relay plus 256 control descriptors. Failure is observable
/// in the extension log; the relay registry still keeps its bounded admission
/// guard when the launchd hard limit is lower.
enum RelayResourceLimits {
    static func prepare() {
        var current = rlimit()
        guard getrlimit(RLIMIT_NOFILE, &current) == 0 else {
            AppLog.warn("NetworkExtension.Resources", "getrlimit(RLIMIT_NOFILE) failed errno=\(errno)")
            return
        }

        let requested = NetworkRelayLimits.maximumActiveRelays
            * NetworkRelayLimits.estimatedFileDescriptorsPerRelay
            + NetworkRelayLimits.reservedFileDescriptors
        let target = min(rlim_t(requested), current.rlim_max)
        guard current.rlim_cur < target else {
            AppLog.info(
                "NetworkExtension.Resources",
                "relay_capacity=\(NetworkRelayLimits.maximumActiveRelays) nofile_soft=\(current.rlim_cur) nofile_hard=\(current.rlim_max)"
            )
            return
        }

        var raised = current
        raised.rlim_cur = target
        guard setrlimit(RLIMIT_NOFILE, &raised) == 0 else {
            AppLog.warn(
                "NetworkExtension.Resources",
                "setrlimit(RLIMIT_NOFILE) failed requested=\(target) current=\(current.rlim_cur) hard=\(current.rlim_max) errno=\(errno)"
            )
            return
        }
        AppLog.info(
            "NetworkExtension.Resources",
            "relay_capacity=\(NetworkRelayLimits.maximumActiveRelays) nofile_soft=\(target) nofile_hard=\(current.rlim_max)"
        )
    }
}
