import ExpoModulesCore
import Foundation
import NetworkExtension

public final class MyProxyModule: Module {
    private let store = MyProxyStore()

    public func definition() -> ModuleDefinition {
        Name("MyProxy")
        AsyncFunction("request") { (requestJSON: String) async -> String in
            await self.request(requestJSON)
        }
    }

    private func request(_ text: String) async -> String {
        guard let data = text.data(using: .utf8),
              let request = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let op = request["op"] as? String else { return failure("invalid_request", "请求格式不正确") }
        switch op {
        case "snapshot", "probe", "select", "setMode", "setAutoConnect", "setFallback", "saveRule", "deleteRule", "removeSource":
            return persistMutation(text)
        case "connect":
            return await connect()
        case "disconnect":
            return await disconnect()
        case "import":
            guard let value = request["text"] as? String, value.utf8.count <= 2_000_000 else { return failure("invalid_import", "节点内容为空或过大") }
            return persistMutation(text)
        case "addSource", "refreshSource":
            guard let urlText = request["url"] as? String, let url = URL(string: urlText), ["http", "https"].contains(url.scheme?.lowercased()) else { return failure("invalid_source", "订阅地址必须是 http 或 https") }
            do {
                let fetched = try await BoundedFetch.fetch(url)
                var enriched = request; enriched["op"] = "import"; enriched["text"] = fetched
                return persistMutation(Self.json(enriched))
            } catch { return failure("source_fetch_failed", "订阅获取失败") }
        default: return failure("unsupported_operation", "暂不支持这个操作")
        }
    }

    private func persistMutation(_ request: String) -> String {
        let response = MyProxyNativeCore.call(request)
        guard Self.isOK(response) else { return response }
        let exported = MyProxyNativeCore.call("{\"op\":\"export\"}")
        guard Self.isOK(exported), let document = Self.data(exported) as? String else { return failure("persist_failed", "配置验证通过，但保存失败") }
        do { try store.write(document) } catch { return failure("persist_failed", "配置保存失败") }
        return response
    }

    private func connect() async -> String {
        let manager = NEVPNManager.shared()
        do {
            try await manager.loadFromPreferences()
            let configuration = NETunnelProviderProtocol()
            configuration.providerBundleIdentifier = "one.leaper.myproxy.xray.PacketTunnel"
            configuration.serverAddress = "MyProxy"
            manager.protocolConfiguration = configuration
            manager.localizedDescription = "MyProxy Xray"
            manager.isEnabled = true
            try await manager.saveToPreferences()
            guard let session = manager.connection as? NETunnelProviderSession else { return failure("vpn_unavailable", "系统 VPN 扩展不可用") }
            try session.startVPNTunnel(options: nil)
            return MyProxyNativeCore.call("{\"op\":\"snapshot\"}")
        } catch { return failure("vpn_start_failed", "无法启动系统 VPN，请检查系统授权") }
    }

    private func disconnect() async -> String {
        let manager = NEVPNManager.shared(); try? await manager.loadFromPreferences(); manager.connection.stopVPNTunnel()
        return MyProxyNativeCore.call("{\"op\":\"snapshot\"}")
    }

    private static func isOK(_ value: String) -> Bool { (try? JSONSerialization.jsonObject(with: Data(value.utf8)) as? [String: Any])?["ok"] as? Bool == true }
    private static func data(_ value: String) -> Any? { (try? JSONSerialization.jsonObject(with: Data(value.utf8)) as? [String: Any])?["data"] }
    private static func json(_ value: [String: Any]) -> String { (try? String(data: JSONSerialization.data(withJSONObject: value), encoding: .utf8)) ?? "{}" }
    private func failure(_ code: String, _ message: String) -> String { Self.json(["ok": false, "error": ["code": code, "message": message]]) }
}

private enum BoundedFetch {
    static func fetch(_ url: URL) async throws -> String {
        var request = URLRequest(url: url); request.timeoutInterval = 15; request.cachePolicy = .reloadIgnoringLocalCacheData
        let (bytes, response) = try await URLSession.shared.bytes(for: request)
        guard let http = response as? HTTPURLResponse, (200..<300).contains(http.statusCode) else { throw URLError(.badServerResponse) }
        var data = Data(); data.reserveCapacity(16 * 1024)
        for try await byte in bytes { data.append(byte); if data.count > 2_000_000 { throw URLError(.dataLengthExceedsMaximum) } }
        return String(decoding: data, as: UTF8.self)
    }
}
