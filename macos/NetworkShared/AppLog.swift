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

    private static var developerForced: Bool {
        switch ProcessInfo.processInfo.environment["MYPROXY_DEV"]?.lowercased() {
        case "1", "true", "yes":
            return true
        default:
            return false
        }
    }

    private static func emit(_ level: AppLogLevel, target: String, message: String) {
        switch level {
        case .error, .warn, .info:
            break
        case .debug, .trace:
            guard developerForced else { return }
        }
        let sanitized = message
            .replacingOccurrences(of: "\n", with: " ")
            .replacingOccurrences(of: "\r", with: " ")
        let ts = UInt64(Date().timeIntervalSince1970) % 86_400
        let h = ts / 3600
        let m = (ts % 3600) / 60
        let s = ts % 60
        let line = "\(pad(h)):\(pad(m)):\(pad(s))Z \(level.rawValue) \(target) \(sanitized)\n"
        write(line)
    }

    private static func pad(_ value: UInt64) -> String {
        value < 10 ? "0\(value)" : "\(value)"
    }

    private static func write(_ line: String) {
        guard let root = FileManager.default.urls(
            for: .applicationSupportDirectory,
            in: .userDomainMask
        ).first else {
            return
        }
        let dir = root.appendingPathComponent("myproxy", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let path = dir.appendingPathComponent("myproxy.log").path
        let fd = open(path, O_WRONLY | O_CREAT | O_APPEND, 0o644)
        guard fd >= 0 else { return }
        flock(fd, LOCK_EX)
        _ = line.withCString { pointer in
            Darwin.write(fd, pointer, strlen(pointer))
        }
        flock(fd, LOCK_UN)
        close(fd)
    }
}
