@preconcurrency import Darwin
@preconcurrency import Foundation
@preconcurrency import NetworkExtension
import MyproxyNetworkShared

/// Owns myproxy's system DNS proxy configuration. NEDNSProxyManager has no
/// start method: saving an enabled provider configuration activates it, and
/// removing that configuration restores the resolver managed by macOS.
actor AppleDNSProxyManager {
    private let providerBundleIdentifier: String
    private var manager: NEDNSProxyManager?

    init(
        providerBundleIdentifier: String = MyproxyNetworkExtensionIdentifiers.systemExtension
    ) {
        self.providerBundleIdentifier = providerBundleIdentifier
    }

    func configureAndEnable(_ bootstrap: Data) async throws {
        let manager = NEDNSProxyManager.shared()
        try await load(manager, operation: .configureDNSProxy)
        try Task.checkCancellation()
        let providerProtocol = NEDNSProxyProviderProtocol()
        providerProtocol.providerBundleIdentifier = providerBundleIdentifier
        providerProtocol.providerConfiguration = [
            "dnsProxyBootstrap": bootstrap
        ]
        manager.providerProtocol = providerProtocol
        manager.localizedDescription = MyproxyNetworkExtensionIdentifiers.localizedDescription
        manager.isEnabled = true
        do {
            try await save(manager, operation: .configureDNSProxy)
        } catch {
            manager.isEnabled = false
            if !isPreferenceTimeout(error) {
                try? await save(manager, operation: .configureDNSProxy)
            }
            throw NetworkExtensionControlFailure(
                operation: .configureDNSProxy,
                underlying: error
            )
        }
        self.manager = manager
    }

    func disable() async throws {
        let manager = NEDNSProxyManager.shared()
        do {
            try await load(manager, operation: .stopDNSProxy)
        } catch {
            // nehelper can stall loadFromPreferences after a killed provider.
            // #68 forbids a bare swallow; only release when getaddrinfo is not
            // still in the fake-ip path.
            try await releaseIfSystemDNSIsClear(cause: error)
            self.manager = nil
            return
        }
        try Task.checkCancellation()
        guard manager.providerProtocol?.providerBundleIdentifier == providerBundleIdentifier else {
            self.manager = nil
            return
        }
        do {
            manager.isEnabled = false
            try await save(manager, operation: .stopDNSProxy)
            try await remove(manager, operation: .stopDNSProxy)
        } catch {
            // Saving disabled is the important resolver restoration step;
            // remove is best effort because macOS may reject it while the
            // provider is stopping. Skip another round-trip when nehelper
            // already timed out.
            if !isPreferenceTimeout(error) {
                manager.isEnabled = false
                try? await save(manager, operation: .stopDNSProxy)
            }
            try await releaseIfSystemDNSIsClear(cause: error)
            self.manager = nil
            return
        }
        self.manager = nil
    }

    /// Preference I/O is not proof. Fake-ip answers are. Public answers mean
    /// NEDNSProxy is not in getaddrinfo, so disconnect can drop :1053.
    private func releaseIfSystemDNSIsClear(cause: Error) async throws {
        switch await probeSystemDNSInterception() {
        case .clear:
            AppLog.warn(
                "ne-host",
                "dns preference I/O failed (\(cause.localizedDescription)); system resolver is not intercepted"
            )
        case .intercepted(let address):
            throw NetworkExtensionControlFailure(
                operation: .stopDNSProxy,
                message: "\(localizedStopReason(cause)); system DNS still intercepted at \(address)"
            )
        case .unproven:
            throw NetworkExtensionControlFailure(
                operation: .stopDNSProxy,
                underlying: cause
            )
        }
    }

    private func load(
        _ manager: NEDNSProxyManager,
        operation: NetworkExtensionControlOperation
    ) async throws {
        try await awaitPreferenceCallback(
            operation: operation,
            api: "loadFromPreferences"
        ) { completion in
            manager.loadFromPreferences(completionHandler: completion)
        }
    }

    private func save(
        _ manager: NEDNSProxyManager,
        operation: NetworkExtensionControlOperation
    ) async throws {
        try await awaitPreferenceCallback(
            operation: operation,
            api: "saveToPreferences"
        ) { completion in
            manager.saveToPreferences(completionHandler: completion)
        }
    }

    private func remove(
        _ manager: NEDNSProxyManager,
        operation: NetworkExtensionControlOperation
    ) async throws {
        try await awaitPreferenceCallback(
            operation: operation,
            api: "removeFromPreferences"
        ) { completion in
            manager.removeFromPreferences(completionHandler: completion)
        }
    }
}

private func isPreferenceTimeout(_ error: Error) -> Bool {
    guard let failure = error as? NetworkExtensionControlFailure else { return false }
    return failure.message.hasPrefix("Timed out waiting for ")
}

private func localizedStopReason(_ error: Error) -> String {
    if let failure = error as? NetworkExtensionControlFailure {
        return failure.message
    }
    return error.localizedDescription
}

private enum SystemDNSInterception: Equatable, Sendable {
    case intercepted(String)
    case clear
    case unproven
}

private func probeSystemDNSInterception() async -> SystemDNSInterception {
    await withTaskGroup(of: SystemDNSInterception.self) { group in
        group.addTask { resolveSystemDNSInterception(host: "example.com") }
        group.addTask {
            try? await Task.sleep(for: .seconds(2))
            return .unproven
        }
        let first = await group.next() ?? .unproven
        group.cancelAll()
        return first
    }
}

private func resolveSystemDNSInterception(host: String) -> SystemDNSInterception {
    var hints = addrinfo()
    hints.ai_family = AF_INET
    hints.ai_socktype = SOCK_STREAM
    var result: UnsafeMutablePointer<addrinfo>?
    let err = getaddrinfo(host, "443", &hints, &result)
    defer {
        if let result {
            freeaddrinfo(result)
        }
    }
    guard err == 0, let first = result else { return .unproven }
    var cursor: UnsafeMutablePointer<addrinfo>? = first
    while let info = cursor {
        if info.pointee.ai_family == AF_INET, let addr = info.pointee.ai_addr {
            var sin = sockaddr_in()
            memcpy(&sin, addr, MemoryLayout<sockaddr_in>.size)
            var buffer = [CChar](repeating: 0, count: Int(INET_ADDRSTRLEN))
            _ = withUnsafePointer(to: sin.sin_addr) { pointer in
                inet_ntop(AF_INET, pointer, &buffer, socklen_t(INET_ADDRSTRLEN))
            }
            let bytes = buffer.prefix { $0 != 0 }.map { UInt8(bitPattern: $0) }
            let ip = String(decoding: bytes, as: UTF8.self)
            if isMihomoFakeIP(ip) {
                return .intercepted(ip)
            }
            return .clear
        }
        cursor = info.pointee.ai_next
    }
    return .unproven
}

/// Clash / mihomo default fake-ip pool is 198.18.0.0/16. Accept the /15 so a
/// custom 198.19.x pool still counts as intercepted.
private func isMihomoFakeIP(_ ip: String) -> Bool {
    let parts = ip.split(separator: ".").compactMap { UInt8($0) }
    guard parts.count == 4 else { return false }
    return parts[0] == 198 && (parts[1] == 18 || parts[1] == 19)
}
