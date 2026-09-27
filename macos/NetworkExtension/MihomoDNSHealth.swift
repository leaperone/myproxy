import Foundation
import MyproxyNetworkShared
@preconcurrency import Network

/// Watches Mihomo's fake-ip DNS listener. The SOCKS backend probe cannot see
/// it: the core may be up while its DNS port is not, and a crashed core would
/// otherwise leave public lookups unanswered until the slower backend probe
/// confirms the outage.
final class MihomoDNSHealth: @unchecked Sendable {
    private static let interval: DispatchTimeInterval = .seconds(10)
    private static let timeout: DispatchTimeInterval = .seconds(2)
    /// `localhost A`, which Mihomo answers from hosts without an upstream.
    private static let query = Data(
        [0x6D, 0x79, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x09]
            + Array("localhost".utf8)
            + [0x00, 0x00, 0x01, 0x00, 0x01]
    )

    private let queue = DispatchQueue(label: "local.harry.myproxy.dns-health")
    private let lock = NSLock()
    private var healthy = true
    private var timer: DispatchSourceTimer?
    private var connection: NWConnection?

    var isHealthy: Bool {
        lock.lock()
        defer { lock.unlock() }
        return healthy
    }

    func start() {
        queue.async { [self] in
            guard timer == nil else { return }
            setHealthy(true)
            let timer = DispatchSource.makeTimerSource(queue: queue)
            timer.schedule(deadline: .now(), repeating: Self.interval)
            timer.setEventHandler { [weak self] in self?.probe() }
            self.timer = timer
            timer.resume()
        }
    }

    func stop() {
        queue.async { [self] in
            timer?.cancel()
            timer = nil
            connection?.cancel()
            connection = nil
        }
    }

    private func probe() {
        connection?.cancel()
        let target = DNSProxyUpstreamResolver.mihomoDNS
        guard let address = target.address.ipAddress,
              let port = NWEndpoint.Port(rawValue: target.port) else { return }
        let connection = NWConnection(
            host: NWEndpoint.Host(address.presentation),
            port: port,
            using: .udp
        )
        self.connection = connection
        let finish: @Sendable (Bool) -> Void = { [weak self] answered in
            guard let self, self.connection === connection else { return }
            self.connection = nil
            connection.cancel()
            self.setHealthy(answered)
        }
        connection.stateUpdateHandler = { state in
            switch state {
            case .ready:
                connection.send(content: Self.query, completion: .contentProcessed { error in
                    if error != nil { finish(false) }
                })
                connection.receiveMessage { data, _, _, error in
                    finish(error == nil && data?.isEmpty == false)
                }
            case .failed, .waiting:
                finish(false)
            default:
                break
            }
        }
        connection.start(queue: queue)
        queue.asyncAfter(deadline: .now() + Self.timeout) { finish(false) }
    }

    private func setHealthy(_ value: Bool) {
        lock.lock()
        healthy = value
        lock.unlock()
    }
}
