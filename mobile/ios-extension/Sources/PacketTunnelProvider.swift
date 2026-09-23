import Foundation
import NetworkExtension

final class PacketTunnelProvider: NEPacketTunnelProvider {
    private let store = MyProxyStore()
    private var engine: MyProxyPacketEngine?

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
        let response = MyProxyNativeCore.call(String(decoding: messageData, as: UTF8.self))
        completionHandler?(Data(response.utf8))
    }

    private static func isOK(_ value: String) -> Bool { (try? JSONSerialization.jsonObject(with: Data(value.utf8)) as? [String: Any])?["ok"] as? Bool == true }
    private static func data(_ value: String) -> Any? { (try? JSONSerialization.jsonObject(with: Data(value.utf8)) as? [String: Any])?["data"] }
    private static func json(_ value: [String: Any]) -> String { (try? String(data: JSONSerialization.data(withJSONObject: value), encoding: .utf8)) ?? "{}" }

    enum TunnelError: Error { case missingDocument, invalidDocument, renderFailed }
}

/// The generated MyProxyNetwork.xcframework supplies this ABI in CI. The
/// provider refuses to start if it is absent; it never reports a fake VPN.
private final class MyProxyPacketEngine {
    private let renderJSON: String
    private weak var packetFlow: NEPacketFlow?
    private var generated: AnyObject?

    init(renderJSON: String, packetFlow: NEPacketFlow) throws {
        self.renderJSON = renderJSON; self.packetFlow = packetFlow
        guard let type = NSClassFromString("MyProxyNetwork.Engine") as? NSObject.Type else { throw PacketEngineError.missingFramework }
        generated = type.init()
    }

    func start() throws {
        guard let generated, let flow = packetFlow else { throw PacketEngineError.notReady }
        guard generated.responds(to: Selector(("startWithRenderJSON:packetFlow:"))) else { throw PacketEngineError.missingABI }
        generated.perform(Selector("startWithRenderJSON:packetFlow:"), with: renderJSON, with: flow)
    }
    func close() { generated?.perform(Selector("close")) }
    enum PacketEngineError: Error { case missingFramework, missingABI, notReady }
}
