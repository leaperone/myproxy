import Foundation

final class MyProxyStore {
    private let url: URL
    init() {
        let root = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: "group.one.leaper.myproxy.xray")!
        try? FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        url = root.appendingPathComponent("myproxy-mobile.document.json")
    }
    func read() throws -> String? { guard FileManager.default.fileExists(atPath: url.path) else { return nil }; return try String(contentsOf: url, encoding: .utf8) }
}
