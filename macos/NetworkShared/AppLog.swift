import Darwin
import Foundation

public enum AppLogLevel: String, Sendable {
    case error
    case warn
    case info
    case debug
    case trace
}

public enum AppLog {
    public static func error(_ target: String, _ message: String) {
        emit(.error, target: target, message: message)
    }

    public static func warn(_ target: String, _ message: String) {
        emit(.warn, target: target, message: message)
    }

    public static func info(_ target: String, _ message: String) {
        emit(.info, target: target, message: message)
    }

    public static func debug(_ target: String, _ message: String) {
        emit(.debug, target: target, message: message)
    }

    /// Lets callers skip building debug-only messages on hot paths.
    public static var isDebugEnabled: Bool { developerForced }

    private static let developerForced: Bool = {
        switch ProcessInfo.processInfo.environment["MYPROXY_DEV"]?.lowercased() {
        case "1", "true", "yes":
            return true
        default:
            return false
        }
    }()

    private static let writer = AppLogFileWriter()

    private static func emit(_ level: AppLogLevel, target: String, message: String) {
        switch level {
        case .error, .warn, .info:
            break
        case .debug, .trace:
            guard developerForced else { return }
        }
        let ts = UInt64(Date().timeIntervalSince1970) % 86_400
        writer.enqueue {
            let sanitized = message
                .replacingOccurrences(of: "\n", with: " ")
                .replacingOccurrences(of: "\r", with: " ")
            let h = ts / 3600
            let m = (ts % 3600) / 60
            let s = ts % 60
            return "\(pad(h)):\(pad(m)):\(pad(s))Z \(level.rawValue) \(target) \(sanitized)\n"
        }
    }

    private static func pad(_ value: UInt64) -> String {
        value < 10 ? "0\(value)" : "\(value)"
    }
}

/// Appends log lines from a private serial queue so callers never block on
/// file I/O. The descriptor stays open between lines; it is reopened when the
/// path no longer names the same file, because the Rust side rotates the log
/// by renaming it. Each append still holds `flock` so lines from the Rust
/// process and this one never interleave.
private final class AppLogFileWriter: @unchecked Sendable {
    private let queue = DispatchQueue(label: "local.harry.myproxy.app-log", qos: .utility)
    private var fd: Int32 = -1
    private var device: dev_t = 0
    private var inode: ino_t = 0
    private lazy var path: String? = Self.resolvePath()

    func enqueue(_ line: @escaping @Sendable () -> String) {
        queue.async { [self] in
            write(line())
        }
    }

    private func write(_ line: String) {
        guard let path else { return }
        if fd >= 0, !stillNamesOpenFile(path) {
            close(fd)
            fd = -1
        }
        if fd < 0 {
            fd = open(path, O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0o644)
            guard fd >= 0 else { return }
            var info = stat()
            if fstat(fd, &info) == 0 {
                device = info.st_dev
                inode = info.st_ino
            }
        }
        flock(fd, LOCK_EX)
        _ = line.withCString { pointer in
            Darwin.write(fd, pointer, strlen(pointer))
        }
        flock(fd, LOCK_UN)
    }

    private func stillNamesOpenFile(_ path: String) -> Bool {
        var info = stat()
        guard stat(path, &info) == 0 else { return false }
        return info.st_dev == device && info.st_ino == inode
    }

    private static func resolvePath() -> String? {
        guard let root = FileManager.default.urls(
            for: .applicationSupportDirectory,
            in: .userDomainMask
        ).first else {
            return nil
        }
        let dir = root.appendingPathComponent("myproxy", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir.appendingPathComponent("myproxy.log").path
    }
}
