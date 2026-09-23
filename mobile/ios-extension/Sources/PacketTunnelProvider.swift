import Foundation
import NetworkExtension
import MyProxyNetwork

final class PacketTunnelProvider: NEPacketTunnelProvider {
    private let store = MyProxyStore()
    private var engine: MobileEngine?

    override func startTunnel(options: [String : NSObject]?, completionHandler: @escaping (Error?) -> Void) {
        do {
            guard let document = try store.read() else { throw TunnelError.missingDocument }
            let loaded = MyProxyNativeCore.call(Self.json(["op": "load", "platform": "ios", "document": document]))
            guard Self.isOK(loaded) else { throw TunnelError.invalidDocument }
            let render = MyProxyNativeCore.call("{\"op\":\"render\"}")
            guard Self.isOK(render), let config = Self.data(render) as? [String: Any], let renderJSON = Self.json(config) else { throw TunnelError.renderFailed }

            let settings = NEPacketTunnelNetworkSettings(tunnelRemoteAddress: "127.0.0.1")
            let ipv4 = NEIPv4Settings(addresses: ["198.18.0.1"], subnetMasks: ["255.255.255.0"])
            ipv4.includedRoutes = [NEIPv4Route.default()]
            settings.ipv4Settings = ipv4
            let ipv6 = NEIPv6Settings(addresses: ["fd00:1::1"], networkPrefixLengths: [126])
            ipv6.includedRoutes = [NEIPv6Route.default()]
            settings.ipv6Settings = ipv6
            settings.dnsSettings = NEDNSSettings(servers: ["1.1.1.1", "9.9.9.9"])
            setTunnelNetworkSettings(settings) { [weak self] error in
                guard let self, error == nil else { completionHandler(error); return }
                do {
                    let adapter = try MyProxyPacketEngine(renderJSON: renderJSON, packetFlow: self.packetFlow)
                    try adapter.start()
                    self.engine = adapter
                    completionHandler(nil)
                } catch { completionHandler(error) }
            }
        } catch { completionHandler(error) }
    }

    override func stopTunnel(with reason: NEProviderStopReason, completionHandler: @escaping () -> Void) {
        engine?.close(); engine = nil; completionHandler()
    }

    override func handleAppMessage(_ messageData: Data, completionHandler: ((Data?) -> Void)? = nil) {
        let request = String(decoding: messageData, as: UTF8.self)
        let response: String
        if request.contains("\"op\":\"snapshot\"") {
            response = engine?.snapshot() ?? MyProxyNativeCore.call(request)
        } else if request.contains("\"op\":\"apply\"") {
            response = restartEngine()
        } else if request.contains("\"op\":\"probe\"") {
            engine?.probe()
            response = engine?.snapshot() ?? MyProxyNativeCore.call(request)
        } else {
            response = MyProxyNativeCore.call(request)
        }
        completionHandler?(Data(response.utf8))
    }

    private func restartEngine() -> String {
        do {
            guard let document = try store.read() else { throw TunnelError.missingDocument }
            engine?.close(); engine = nil
            let loaded = MyProxyNativeCore.call(Self.json(["op": "load", "platform": "ios", "document": document]))
            guard Self.isOK(loaded) else { throw TunnelError.invalidDocument }
            let render = MyProxyNativeCore.call("{\"op\":\"render\"}")
            guard Self.isOK(render), let config = Self.data(render) as? [String: Any], let renderJSON = Self.json(config) else { throw TunnelError.renderFailed }
            let newEngine = try MyProxyPacketEngine(renderJSON: renderJSON, packetFlow: packetFlow)
            try newEngine.start(); engine = newEngine
            return newEngine.snapshot()
        } catch { return "{\"ok\":false,\"error\":{\"code\":\"apply_failed\",\"message\":\"配置已保存，但 VPN 应用失败\"}}" }
    }

    private static func isOK(_ value: String) -> Bool { (try? JSONSerialization.jsonObject(with: Data(value.utf8)) as? [String: Any])?["ok"] as? Bool == true }
    private static func data(_ value: String) -> Any? { (try? JSONSerialization.jsonObject(with: Data(value.utf8)) as? [String: Any])?["data"] }
    private static func json(_ value: [String: Any]) -> String { (try? String(data: JSONSerialization.data(withJSONObject: value), encoding: .utf8)) ?? "{}" }

    enum TunnelError: Error { case missingDocument, invalidDocument, renderFailed }
}

private final class MyProxyPacketEngine: NSObject, MobilePolicyProtocol, MobileProtectorProtocol, MobilePacketWriterProtocol {
    private let packetFlow: NEPacketTunnelFlow
    private var engine: MobileEngine?
    private var readerRunning = true

    init(renderJSON: String, packetFlow: NEPacketTunnelFlow) throws {
        self.packetFlow = packetFlow
        super.init()
        var error: NSError?
        guard let value = MobileNewEngine(renderJSON, self, self, false, &error) else {
            throw error ?? PacketEngineError.engineUnavailable
        }
        self.engine = value
    }

    func start() throws {
        guard let engine else { throw PacketEngineError.engineUnavailable }
        var error: NSError?
        engine.start(self, error: &error)
        if let error { throw error }
        readPackets()
    }

    func close() {
        readerRunning = false
        engine?.close()
        engine = nil
    }

    func closeConnections() { engine?.closeConnections() }
    func snapshot() -> String { engine?.snapshot() ?? "{\"phase\":\"disconnected\"}" }
    func probe() { engine?.probe() }

    func decide(_ requestJSON: String) -> String { MyProxyNativeCore.call(requestJSON) }
    func health(_ node: String, delayMs: Int64, failed: Bool) { _ = MyProxyNativeCore.call(Self.healthRequest(node, delayMs, failed)) }
    func protect(_ fd: Int64) -> Bool { true }
    func writePacket(_ packet: Data) -> Bool {
        packetFlow.writePackets([packet], withProtocols: [.IPv4])
        return true
    }

    private func readPackets() {
        guard readerRunning else { return }
        packetFlow.readPackets { [weak self] packets, protocols in
            guard let self, self.readerRunning else { return }
            for (index, packet) in packets.enumerated() {
                guard let engine = self.engine else { return }
                var error: NSError?
                engine.writePacket(packet, error: &error)
                if error != nil { self.close(); return }
                _ = protocols[index]
            }
            self.readPackets()
        }
    }

    private static func healthRequest(_ node: String, _ delay: Int64, _ failed: Bool) -> String {
        "{\"op\":\"health\",\"node\":\"\(node.replacingOccurrences(of: "\\\"", with: ""))\",\"delayMs\":\(delay),\"failed\":\(failed ? "true" : "false")}"
    }

    enum PacketEngineError: Error { case engineUnavailable }
}
