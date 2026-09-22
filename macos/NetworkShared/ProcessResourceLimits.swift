import Darwin
import Foundation
import OSLog

/// Raises only this process's soft file-descriptor limit for the Xray channel.
/// The hard limit and every other process remain untouched.
public enum ProcessResourceLimits {
    public static let xrayNoFileTarget: rlim_t = 16_384

    public struct Snapshot: Sendable {
        public let soft: rlim_t
        public let hard: rlim_t
    }
    public struct Errno: Error, Sendable { public let value: Int32 }

    private static let logger = Logger(
        subsystem: "local.harry.myproxy.network-extension",
        category: "ProcessResourceLimits"
    )

    public static func snapshot() -> Result<Snapshot, Errno> {
        var limits = rlimit()
        guard getrlimit(RLIMIT_NOFILE, &limits) == 0 else {
            return .failure(Errno(value: errno))
        }
        return .success(Snapshot(soft: limits.rlim_cur, hard: limits.rlim_max))
    }

    @discardableResult
    public static func raiseXraySoftLimit(
        target: rlim_t = xrayNoFileTarget
    ) -> Result<Snapshot, Errno> {
        guard case let .success(before) = snapshot() else {
            let error = errno
            logger.error("getrlimit NOFILE failed errno=\(error, privacy: .public)")
            return .failure(Errno(value: error))
        }
        let desired = min(target, before.hard)
        guard desired > before.soft else {
            logger.info("NOFILE unchanged soft=\(before.soft, privacy: .public) hard=\(before.hard, privacy: .public)")
            return .success(before)
        }
        var limits = rlimit(rlim_cur: desired, rlim_max: before.hard)
        guard setrlimit(RLIMIT_NOFILE, &limits) == 0 else {
            let error = errno
            logger.error("setrlimit NOFILE failed before=\(before.soft, privacy: .public) hard=\(before.hard, privacy: .public) errno=\(error, privacy: .public)")
            return .failure(Errno(value: error))
        }
        guard case let .success(after) = snapshot() else {
            let error = errno
            logger.error("getrlimit NOFILE after set failed errno=\(error, privacy: .public)")
            return .failure(Errno(value: error))
        }
        logger.info("NOFILE raised soft=\(before.soft, privacy: .public)->\(after.soft, privacy: .public) hard=\(after.hard, privacy: .public)")
        return .success(after)
    }
}
