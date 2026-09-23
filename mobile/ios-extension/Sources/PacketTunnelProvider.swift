import Foundation
import NetworkExtension
import MyProxyNetwork
import Darwin

final class PacketTunnelProvider: NEPacketTunnelProvider {
    private let store = MyProxyStore()
    private let commandLock = NSRecursiveLock()
    private var lifecycleEpoch: UInt64 = 0
    private var engine: MyProxyPacketEngine?
    private var monitor: DispatchSourceTimer?

    override func startTunnel(options: [String : NSObject]?, completionHandler: @escaping (Error?) -> Void) {
        commandLock.lock(); lifecycleEpoch &+= 1; let epoch = lifecycleEpoch; commandLock.unlock()
        do {
            guard let document = try store.read() else { throw TunnelError.missingDocument }
            let loaded = MyProxyNativeCore.call(Self.json(["op": "load", "platform": "ios", "document": document]))
            guard Self.isOK(loaded) else { throw TunnelError.invalidDocument }
            let render = MyProxyNativeCore.call("{\"op\":\"render\"}")
            guard Self.isOK(render), let config = Self.data(render) as? [String: Any] else { throw TunnelError.renderFailed }
            let renderJSON = Self.json(config)

            let settings = NEPacketTunnelNetworkSettings(tunnelRemoteAddress: "127.0.0.1")
            let ipv4 = NEIPv4Settings(addresses: ["198.18.0.1"], subnetMasks: ["255.255.255.0"])
            ipv4.includedRoutes = [NEIPv4Route.default()]
            ipv4.excludedRoutes = [
                NEIPv4Route(destinationAddress: "10.0.0.0", subnetMask: "255.0.0.0"),
                NEIPv4Route(destinationAddress: "172.16.0.0", subnetMask: "255.240.0.0"),
                NEIPv4Route(destinationAddress: "192.168.0.0", subnetMask: "255.255.0.0"),
                NEIPv4Route(destinationAddress: "127.0.0.0", subnetMask: "255.0.0.0"),
                NEIPv4Route(destinationAddress: "169.254.0.0", subnetMask: "255.255.0.0")
            ]
            settings.ipv4Settings = ipv4
            let ipv6 = NEIPv6Settings(addresses: ["fd00:1::1"], networkPrefixLengths: [126])
            ipv6.includedRoutes = [NEIPv6Route.default()]
            ipv6.excludedRoutes = [
                NEIPv6Route(destinationAddress: "fc00::", networkPrefixLength: 7),
                NEIPv6Route(destinationAddress: "fe80::", networkPrefixLength: 10),
                NEIPv6Route(destinationAddress: "::1", networkPrefixLength: 128)
            ]
            settings.ipv6Settings = ipv6
            settings.dnsSettings = NEDNSSettings(servers: ["1.1.1.1", "9.9.9.9"])
            setTunnelNetworkSettings(settings) { [weak self] error in
                guard let self, error == nil else { completionHandler(error); return }
                self.commandLock.lock()
                guard epoch == self.lifecycleEpoch else {
                    self.commandLock.unlock()
                    completionHandler(TunnelError.cancelled)
                    return
                }
                self.commandLock.unlock()
                do {
                    let adapter = try MyProxyPacketEngine(renderJSON: renderJSON, packetFlow: self.packetFlow)
                    try adapter.start()
                    self.commandLock.lock()
                    guard epoch == self.lifecycleEpoch else {
                        self.commandLock.unlock()
                        adapter.close()
                        completionHandler(TunnelError.cancelled)
                        return
                    }
                    self.engine = adapter
                    self.startMonitoring()
                    self.commandLock.unlock()
                    completionHandler(nil)
                } catch { completionHandler(error) }
            }
        } catch { completionHandler(error) }
    }

    override func stopTunnel(with reason: NEProviderStopReason, completionHandler: @escaping () -> Void) {
        commandLock.lock(); defer { commandLock.unlock() }
        lifecycleEpoch &+= 1
        monitor?.cancel(); monitor = nil
        engine?.close(); engine = nil; completionHandler()
    }

    private func startMonitoring() {
        monitor?.cancel()
        let timer = DispatchSource.makeTimerSource(queue: .global(qos: .utility))
        let epoch = lifecycleEpoch
        timer.schedule(deadline: .now() + 2, repeating: 2)
        timer.setEventHandler { [weak self] in
            guard let self else { return }
            self.commandLock.lock(); defer { self.commandLock.unlock() }
            guard self.lifecycleEpoch == epoch, let current = self.engine else { return }
            let data = Data(current.snapshot().utf8)
            let runtime = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
            if runtime?["phase"] as? String == "disconnected" {
                self.cancelTunnelWithError(TunnelError.engineStopped)
            }
        }
        monitor = timer
        timer.resume()
    }

    override func handleAppMessage(_ messageData: Data, completionHandler: ((Data?) -> Void)? = nil) {
        commandLock.lock(); defer { commandLock.unlock() }
        let request = String(decoding: messageData, as: UTF8.self)
        let response: String
        if request.contains("\"op\":\"snapshot\"") {
            response = providerSnapshot()
        } else if request.contains("\"op\":\"apply\"") {
            response = restartEngine()
        } else if request.contains("\"op\":\"probe\"") {
            engine?.probe()
            response = providerSnapshot()
        } else {
            response = MyProxyNativeCore.call(request)
        }
        completionHandler?(Data(response.utf8))
    }

    private func restartEngine() -> String {
        let oldDocument = MyProxyNativeCore.call("{\"op\":\"export\"}")
        guard Self.isOK(oldDocument), let oldText = Self.data(oldDocument) as? String else { return applyFailure() }
        let oldRender: String
        do { oldRender = try currentRenderJSON() } catch { return applyFailure() }
        var oldClosed = false
        do {
            guard let document = try store.read() else { throw TunnelError.missingDocument }
            let loaded = MyProxyNativeCore.call(Self.json(["op": "load", "platform": "ios", "document": document]))
            guard Self.isOK(loaded) else { throw TunnelError.invalidDocument }
            let candidate = try currentRenderJSON()
            var validationError: NSError?
            MobileValidate(candidate, &validationError)
            if let validationError { throw validationError }

            let oldEngine = engine
            engine = nil
            oldEngine?.close()
            oldClosed = true
            let newEngine = try MyProxyPacketEngine(renderJSON: candidate, packetFlow: packetFlow)
            try newEngine.start()
            engine = newEngine
            return providerSnapshot()
        } catch {
            _ = MyProxyNativeCore.call(Self.json(["op": "load", "platform": "ios", "document": oldText]))
            if oldClosed {
                do {
                    let restored = try MyProxyPacketEngine(renderJSON: oldRender, packetFlow: packetFlow)
                    try restored.start(); engine = restored
                    return applyFailure()
                } catch {
                    cancelTunnelWithError(error)
                }
            }
            return applyFailure()
        }
    }

    private func currentRenderJSON() throws -> String {
        let render = MyProxyNativeCore.call("{\"op\":\"render\"}")
        guard Self.isOK(render), let config = Self.data(render) as? [String: Any] else { throw TunnelError.renderFailed }
        return Self.json(config)
    }

    private func applyFailure() -> String { "{\"ok\":false,\"error\":{\"code\":\"apply_failed\",\"message\":\"配置已保存，但 VPN 应用失败\"}}" }

    private func providerSnapshot() -> String {
        let base = MyProxyNativeCore.call("{\"op\":\"snapshot\"}")
        guard let data = base.data(using: .utf8), var root = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any], var payload = root["data"] as? [String: Any], let runtimeJSON = engine?.snapshot(), let runtimeData = runtimeJSON.data(using: .utf8), let runtime = (try? JSONSerialization.jsonObject(with: runtimeData)) as? [String: Any] else { return base }
        payload["runtime"] = runtime; root["data"] = payload
        return (try? String(data: JSONSerialization.data(withJSONObject: root), encoding: .utf8)) ?? base
    }

    private static func isOK(_ value: String) -> Bool { (try? JSONSerialization.jsonObject(with: Data(value.utf8)) as? [String: Any])?["ok"] as? Bool == true }
    private static func data(_ value: String) -> Any? { (try? JSONSerialization.jsonObject(with: Data(value.utf8)) as? [String: Any])?["data"] }
    private static func json(_ value: [String: Any]) -> String { (try? String(data: JSONSerialization.data(withJSONObject: value), encoding: .utf8)) ?? "{}" }

    enum TunnelError: Error { case missingDocument, invalidDocument, renderFailed, cancelled, engineStopped }
}

private final class MyProxyPacketEngine: NSObject, MobilePolicyProtocol, MobilePacketWriterProtocol {
    private let packetFlow: NEPacketTunnelFlow
    private let stateLock = NSLock()
    private var engine: MobileEngine?
    private var readerRunning = true

    init(renderJSON: String, packetFlow: NEPacketTunnelFlow) throws {
        self.packetFlow = packetFlow
        super.init()
        var error: NSError?
        guard let value = MobileNewEngine(renderJSON, self, nil, false, &error) else {
            throw error ?? PacketEngineError.engineUnavailable
        }
        self.engine = value
        guard let data = renderJSON.data(using: .utf8),
              let render = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              let revision = render["revision"] as? NSNumber else {
            value.close()
            throw PacketEngineError.engineUnavailable
        }
        let command = try JSONSerialization.data(withJSONObject: ["op": "activate", "revision": revision])
        let response = MyProxyNativeCore.call(String(decoding: command, as: UTF8.self))
        guard let responseData = response.data(using: .utf8),
              let envelope = try JSONSerialization.jsonObject(with: responseData) as? [String: Any], envelope["ok"] as? Bool == true else {
            value.close()
            throw PacketEngineError.engineUnavailable
        }
    }

    func start() throws {
        guard let engine else { throw PacketEngineError.engineUnavailable }
        try engine.start(self)
        readPackets()
    }

    func close() {
        stateLock.lock(); readerRunning = false; let oldEngine = engine; engine = nil; stateLock.unlock()
        oldEngine?.close()
    }

    func closeConnections() { stateLock.lock(); let current = engine; stateLock.unlock(); current?.closeConnections() }
    func snapshot() -> String { stateLock.lock(); let current = engine; stateLock.unlock(); return current?.snapshot() ?? "{\"phase\":\"disconnected\"}" }
    func probe() { stateLock.lock(); let current = engine; stateLock.unlock(); current?.probe() }

    func decide(_ requestJSON: String?) -> String { MyProxyNativeCore.call(requestJSON ?? "{}") }
    func health(_ node: String?, delayMs: Int64, failed: Bool) { _ = MyProxyNativeCore.call(Self.healthRequest(node ?? "", delayMs, failed)) }
    func writePacket(_ packet: Data?) -> Bool {
        guard let packet else { return false }
        let version = packet.first.map { $0 >> 4 } ?? 0
        let family = version == 6 ? AF_INET6 : AF_INET
        return packetFlow.writePackets([packet], withProtocols: [NSNumber(value: family)])
    }

    private func readPackets() {
        stateLock.lock(); let running = readerRunning; stateLock.unlock()
        guard running else { return }
        packetFlow.readPackets { [weak self] packets, protocols in
            guard let self else { return }
            self.stateLock.lock(); let running = self.readerRunning; let current = self.engine; self.stateLock.unlock()
            guard running, let current else { return }
            for (index, packet) in packets.enumerated() {
                do { try current.writePacket(packet) } catch { self.close(); return }
                _ = protocols[index]
            }
            self.readPackets()
        }
    }

    private static func healthRequest(_ node: String, _ delay: Int64, _ failed: Bool) -> String {
        let value: [String: Any] = ["op": "health", "node": node, "delayMs": delay, "failed": failed]
        guard let data = try? JSONSerialization.data(withJSONObject: value) else { return "{}" }
        return String(decoding: data, as: UTF8.self)
    }

    enum PacketEngineError: Error { case engineUnavailable }
}
