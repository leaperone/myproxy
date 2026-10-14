import Foundation

final class MyProxyStore {
    private let url: URL

    init() {
        let root = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: "group.one.leaper.myproxy.xray")
            ?? FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        try? FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        url = root.appendingPathComponent("myproxy-mobile.document.json")
    }

    func read() throws -> String? {
        guard FileManager.default.fileExists(atPath: url.path) else { return nil }
        return try String(contentsOf: url, encoding: .utf8)
    }

    func write(_ text: String) throws {
        let tmp = url.deletingLastPathComponent().appendingPathComponent(".myproxy-mobile.document.json.tmp")
        try text.write(to: tmp, atomically: true, encoding: .utf8)
        if FileManager.default.fileExists(atPath: url.path) {
            _ = try FileManager.default.replaceItemAt(url, withItemAt: tmp, backupItemName: nil, options: .usingNewMetadataOnly)
        } else {
            try FileManager.default.moveItem(at: tmp, to: url)
        }
    }
}
