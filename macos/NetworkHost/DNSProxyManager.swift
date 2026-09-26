@preconcurrency import Darwin
@preconcurrency import Foundation
import MyproxyNetworkShared
@preconcurrency import NetworkExtension

enum SystemDNSProbe: Sendable, Equatable {
    case intercepted
    case clear
    case unproven
}

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

    /// `getaddrinfo` is the path NEDNSProxy actually intercepts. A SOCKS
    /// probe of :1053 can succeed while this call hangs.
    func proveSystemResolver() async -> SystemDNSProbe {
        #if MYPROXY_XRAY
        // mDNSResponder owns this lookup, and the first one races Xray's
        // listener. One 3s miss was disabling system capture while DNS bytes
        // were already moving.
        var last = await probeSystemResolverOnce(timeout: 5)
        if last == .unproven {
            AppLog.info("ne-host", "system resolver probe=unproven; retrying")
            try? await Task.sleep(for: .milliseconds(400))
            last = await probeSystemResolverOnce(timeout: 5)
        }
        #else
        var last = await probeSystemResolverOnce(timeout: 3)
        #endif
        guard last == .clear else { return last }
        for _ in 0..<3 {
            try? await Task.sleep(for: .milliseconds(400))
            last = await probeSystemResolverOnce(timeout: 3)
            if last != .clear { return last }
        }
        return last
    }

    func disable() async throws {
        let manager = NEDNSProxyManager.shared()
        try await load(manager, operation: .stopDNSProxy)
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
            self.manager = manager
            throw NetworkExtensionControlFailure(
                operation: .stopDNSProxy,
                underlying: error
            )
        }
        self.manager = nil
    }

    private func probeSystemResolverOnce(timeout: TimeInterval) async -> SystemDNSProbe {
        let reply = OnceReply<SystemDNSProbe>()
        do {
            return try await withCheckedThrowingContinuation { continuation in
                reply.attach(continuation)
                Thread.detachNewThread {
                    reply.finish(.success(resolveExampleComARecord()))
                }
                DispatchQueue.global().asyncAfter(deadline: .now() + timeout) {
                    reply.finish(.success(.unproven))
                }
            }
        } catch {
            return .unproven
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

private func resolveExampleComARecord() -> SystemDNSProbe {
    // A cached example.com answer returns before NEDNSProxy is on the path and
    // lets capture stay up while later lookups hang. A fresh name has to reach
    // the resolver; NXDOMAIN still proves that it answered.
    let name = "myproxy-\(UUID().uuidString.lowercased()).example.com"
    var hints = addrinfo()
    hints.ai_family = AF_INET
    hints.ai_socktype = SOCK_STREAM
    var result: UnsafeMutablePointer<addrinfo>?
    let status = name.withCString { pointer in
        getaddrinfo(pointer, nil, &hints, &result)
    }
    defer {
        if let result {
            freeaddrinfo(result)
        }
    }
    if status == EAI_NONAME {
        return .clear
    }
    guard status == 0 else { return .unproven }
    var cursor = result
    var sawAddress = false
    while let info = cursor {
        if info.pointee.ai_family == AF_INET, let addr = info.pointee.ai_addr {
            sawAddress = true
            let ipv4 = addr.withMemoryRebound(to: sockaddr_in.self, capacity: 1) { pointer in
                UInt32(bigEndian: pointer.pointee.sin_addr.s_addr)
            }
            if ipv4 >> 16 == 0xC612 {
                return .intercepted
            }
        }
        cursor = info.pointee.ai_next
    }
    return sawAddress ? .clear : .unproven
}
