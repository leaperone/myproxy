import ExpoModulesCore
import Foundation
import NetworkExtension

public final class MyProxyModule: Module {
    private let store = MyProxyStore()
    private let mutationLock = NSLock()

    public func definition() -> ModuleDefinition {
        Name("MyProxy")
        OnCreate {
            self.loadCore()
        }
        AsyncFunction("request") { (requestJSON: String) async -> String in
            await self.request(requestJSON)
        }
    }

    private func loadCore() {
        do {
            if let document = try store.read() {
                _ = MyProxyNativeCore.call(Self.json(["op": "load", "platform": "ios", "document": document]))
            } else { _ = MyProxyNativeCore.call("{\"op\":\"init\",\"platform\":\"ios\"}") }
        } catch { _ = MyProxyNativeCore.call("{\"op\":\"init\",\"platform\":\"ios\"}") }
    }

    private func request(_ text: String) async -> String {
        guard let data = text.data(using: .utf8),
              let request = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let op = request["op"] as? String else { return failure("invalid_request", "请求格式不正确") }
        switch op {
        case "snapshot": return await snapshot()
        case "select", "setMode", "setAutoConnect", "setFallback", "saveRule", "deleteRule", "removeSource":
            return await persistMutation(text)
        case "probe":
            if let providerResponse = await sendProvider("{\"op\":\"probe\"}"), !Self.isOK(providerResponse) { return failure("probe_failed", "节点检测失败") }
            return await snapshot()
        case "connect":
            return await connect()
        case "disconnect":
            return await disconnect()
        case "import":
            guard let value = request["text"] as? String, value.utf8.count <= 2_000_000 else { return failure("invalid_import", "节点内容为空或过大") }
            return await persistMutation(text)
        case "addSource", "refreshSource":
            let existing = sourceInfo(id: request["id"] as? String)
            let urlText = (request["url"] as? String) ?? existing?.url
            guard let urlText, let url = URL(string: urlText), ["http", "https"].contains(url.scheme?.lowercased()) else { return failure("invalid_source", "找不到有效的订阅地址") }
            do {
                let fetched = try await BoundedFetch.fetch(url)
                var enriched = request; enriched["op"] = "import"; enriched["text"] = fetched
                enriched["sourceURL"] = urlText
                if let id = request["id"] as? String { enriched["sourceId"] = id }
                if enriched["name"] == nil, let name = existing?.name { enriched["name"] = name }
                return await persistMutation(Self.json(enriched))
            } catch { return failure("source_fetch_failed", "订阅获取失败") }
        default: return failure("unsupported_operation", "暂不支持这个操作")
        }
    }

    private func persistMutation(_ request: String) async -> String {
        mutationLock.lock(); defer { mutationLock.unlock() }
        let previous: String?
        do { previous = try store.read() } catch { previous = nil }
        let response = MyProxyNativeCore.call(request)
        guard Self.isOK(response) else { return response }
        let exported = MyProxyNativeCore.call("{\"op\":\"export\"}")
        guard Self.isOK(exported), let document = Self.data(exported) as? String else { return failure("persist_failed", "配置验证通过，但保存失败") }
        do { try store.write(document) } catch {
            if let previous { _ = MyProxyNativeCore.call(Self.json(["op": "load", "platform": "ios", "document": previous])) }
            return failure("persist_failed", "配置保存失败")
        }
        let providerOp = request.contains("\"op\":\"probe\"") ? "probe" : "apply"
        if let providerResponse = await sendProvider(Self.json(["op": providerOp])), !Self.isOK(providerResponse) {
            return failure("apply_failed", "配置已保存，但 VPN 应用失败")
        }
        return response
    }

    private func connect() async -> String {
        do {
            let manager = try await vpnManager()
            let configuration = NETunnelProviderProtocol()
            configuration.providerBundleIdentifier = "one.leaper.myproxy.xray.PacketTunnel"
            configuration.serverAddress = "MyProxy"
            manager.protocolConfiguration = configuration
            manager.localizedDescription = "MyProxy Xray"
            manager.isEnabled = true
            try await manager.saveToPreferences()
            let refreshed = try await vpnManager()
            guard let session = refreshed.connection as? NETunnelProviderSession else { return failure("vpn_unavailable", "系统 VPN 扩展不可用") }
            try session.startVPNTunnel(options: nil)
            return await snapshot()
        } catch { return failure("vpn_start_failed", "无法启动系统 VPN，请检查系统授权") }
    }

    private func disconnect() async -> String {
        if let manager = try? await vpnManager() { manager.connection.stopVPNTunnel() }
        return MyProxyNativeCore.call("{\"op\":\"snapshot\"}")
    }

    private func snapshot() async -> String {
        let local = MyProxyNativeCore.call("{\"op\":\"snapshot\"}")
        guard let manager = try? await vpnManager(), let session = manager.connection as? NETunnelProviderSession else { return local }
        if session.status == .connecting || session.status == .reasserting { return withRuntime(local, phase: "connecting") }
        if session.status == .disconnecting { return withRuntime(local, phase: "disconnecting") }
        guard session.status == .connected else { return local }
        return await sendProvider("{\"op\":\"snapshot\"}") ?? local
    }

    private func sendProvider(_ request: String) async -> String? {
        guard let manager = try? await vpnManager(), let session = manager.connection as? NETunnelProviderSession else { return nil }
        return await withCheckedContinuation { (continuation: CheckedContinuation<String?, Never>) in
            let lock = NSLock(); var finished = false
            let finish: (String?) -> Void = { value in
                lock.lock(); defer { lock.unlock() }
                guard !finished else { return }; finished = true; continuation.resume(returning: value)
            }
            do {
                try session.sendProviderMessage(Data(request.utf8)) { data in
                    finish(data.map { String(decoding: $0, as: UTF8.self) })
                }
            } catch { finish(nil) }
            DispatchQueue.global().asyncAfter(deadline: .now() + 3) { finish(nil) }
        }
    }

    private func withRuntime(_ response: String, phase: String) -> String {
        guard let data = response.data(using: .utf8), var root = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any], var payload = root["data"] as? [String: Any] else { return response }
        payload["runtime"] = ["phase": phase, "message": nil, "connectedAt": nil, "uploadBytes": 0, "downloadBytes": 0, "connections": []]
        root["data"] = payload
        return (try? String(data: JSONSerialization.data(withJSONObject: root), encoding: .utf8)) ?? response
    }

    private func vpnManager() async throws -> NETunnelProviderManager {
        let managers: [NETunnelProviderManager] = try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<[NETunnelProviderManager], Error>) in
            NETunnelProviderManager.loadAllFromPreferences { managers, error in
                if let error { continuation.resume(throwing: error) } else { continuation.resume(returning: managers ?? []) }
            }
        }
        if let existing = managers.first(where: { ($0.protocolConfiguration as? NETunnelProviderProtocol)?.providerBundleIdentifier == "one.leaper.myproxy.xray.PacketTunnel" }) {
            return existing
        }
        let manager = NETunnelProviderManager(); manager.localizedDescription = "MyProxy Xray"; return manager
    }

    private func sourceInfo(id: String?) -> (url: String, name: String)? {
        guard let id, let data = MyProxyNativeCore.call("{\"op\":\"snapshot\"}").data(using: .utf8),
              let root = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let sources = root["data"] as? [String: Any], let rows = sources["sources"] as? [[String: Any]] else { return nil }
        guard let row = rows.first(where: { $0["id"] as? String == id }), let url = row["url"] as? String else { return nil }
        return (url, row["name"] as? String ?? "订阅")
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
