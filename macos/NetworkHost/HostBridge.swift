import Foundation
import MyproxyNetworkShared

private struct HostEnableRequest: Decodable, Sendable {
    struct ProcessRule: Decodable, Sendable {
        let pattern: String
        let via: String
    }

    struct GroupPort: Decodable, Sendable {
        let name: String
        let port: UInt16
    }

    let revision: UInt64
    let socksPort: UInt16
    let username: String
    let password: String
    let processRules: [ProcessRule]
    let groupPorts: [GroupPort]
}

private enum HostEnableOutcome {
    case running
    case requiresReboot
}

private actor HostController {
    static let shared = HostController()

    private let systemExtension = AppleSystemExtensionController()
    private let transparentProxy = AppleTransparentProxyManager()
    private let dnsProxy = AppleDNSProxyManager()
    private var lastEndpoints: [MihomoRouteProxyEndpoint] = []
    private var lastSocksPort: UInt16?
    private var lastUsername: String?
    private var lastPassword: String?

    func enable(json: String) async throws -> HostEnableOutcome {
        let request = try JSONDecoder().decode(
            HostEnableRequest.self,
            from: Data(json.utf8)
        )
        let endpoints = try routeEndpoints(from: request)
        let activationIdentifier = UUID()
        let configurations = try providerConfigurations(
            from: request,
            endpoints: endpoints,
            activationIdentifier: activationIdentifier
        )
        let outcome = try await systemExtension.activate()
        if case .requiresReboot = outcome {
            return .requiresReboot
        }
        let canLiveUpdate = await transparentProxy.isConnected()
            && lastSocksPort == request.socksPort
            && lastUsername == request.username
            && lastPassword == request.password
            && preservesRouteEndpoints(lastEndpoints, endpoints)
        if canLiveUpdate {
            do {
                try await transparentProxy.configureAndApplyRunning(
                    configurations.transparent,
                    revision: request.revision
                )
                try await transparentProxy.prepareDNS(
                    revision: request.revision,
                    activationIdentifier: activationIdentifier,
                    bootstrap: configurations.dnsBootstrap
                )
                do {
                    try await dnsProxy.configureAndEnable(configurations.dnsBootstrap)
                } catch {
                    try? await dnsProxy.disable()
                    try? await transparentProxy.stop()
                    throw error
                }
                remember(request, endpoints: endpoints)
                return .running
            } catch {
                try? await dnsProxy.disable()
                try? await transparentProxy.stop()
                try await transparentProxy.configure(configurations.transparent)
                try await transparentProxy.start()
                try await transparentProxy.prepareDNS(
                    revision: request.revision,
                    activationIdentifier: activationIdentifier,
                    bootstrap: configurations.dnsBootstrap
                )
                do {
                    try await dnsProxy.configureAndEnable(configurations.dnsBootstrap)
                } catch {
                    try? await dnsProxy.disable()
                    try? await transparentProxy.stop()
                    throw error
                }
                remember(request, endpoints: endpoints)
                return .running
            }
        }
        try? await dnsProxy.disable()
        try? await transparentProxy.stop()
        try await transparentProxy.configure(configurations.transparent)
        try await transparentProxy.start()
        do {
            try await transparentProxy.prepareDNS(
                revision: request.revision,
                activationIdentifier: activationIdentifier,
                bootstrap: configurations.dnsBootstrap
            )
            try await dnsProxy.configureAndEnable(configurations.dnsBootstrap)
        } catch {
            try? await dnsProxy.disable()
            try? await transparentProxy.stop()
            throw error
        }
        remember(request, endpoints: endpoints)
        return .running
    }

    func disable() async throws {
        var firstError: Error?
        do { try await dnsProxy.disable() } catch { firstError = error }
        do { try await transparentProxy.stop() } catch { if firstError == nil { firstError = error } }
        lastEndpoints = []
        lastSocksPort = nil
        lastUsername = nil
        lastPassword = nil
        if let firstError { throw firstError }
    }

    private func remember(
        _ request: HostEnableRequest,
        endpoints: [MihomoRouteProxyEndpoint]
    ) {
        lastEndpoints = endpoints
        lastSocksPort = request.socksPort
        lastUsername = request.username
        lastPassword = request.password
    }
}

private func preservesRouteEndpoints(
    _ previous: [MihomoRouteProxyEndpoint],
    _ next: [MihomoRouteProxyEndpoint]
) -> Bool {
    previous.allSatisfy { old in
        next.contains { candidate in
            candidate.route == old.route
                && candidate.host == old.host
                && candidate.port == old.port
        }
    }
}

private struct HostProviderConfigurations: @unchecked Sendable {
    let transparent: [String: NSObject]
    let dnsBootstrap: Data
}

private func providerConfigurations(
    from request: HostEnableRequest,
    endpoints: [MihomoRouteProxyEndpoint],
    activationIdentifier: UUID
) throws -> HostProviderConfigurations {
    let snapshot = try captureSnapshot(from: request)
    let encoder = JSONEncoder()
    encoder.dateEncodingStrategy = .iso8601
    let encodedSnapshot = try encoder.encode(snapshot)
    let catalog = try MihomoRouteProxyCatalog.encode(endpoints)
    let bootstrap = try DNSProxyBootstrapConfiguration(
        revision: request.revision,
        activationIdentifier: activationIdentifier,
        profileRulesProxy: endpoints[0],
        routeProxyEndpoints: endpoints,
        encodedCaptureSnapshot: encodedSnapshot
    ).encoded()
    let transparent: [String: NSObject] = [
        "revision": NSNumber(value: request.revision),
        "activationIdentifier": activationIdentifier.uuidString as NSString,
        "dnsProxyBootstrap": bootstrap as NSData,
        "captureEnabled": NSNumber(value: true),
        "failOpen": NSNumber(value: true),
        "captureConfigurationSnapshot": encodedSnapshot as NSData,
        "mihomoRouteProxyCatalog": catalog as NSData,
        "mihomoSOCKSHost": "127.0.0.1" as NSString,
        "mihomoSOCKSPort": NSNumber(value: request.socksPort),
        "mihomoSOCKSUsername": request.username as NSString,
        "mihomoSOCKSPassword": request.password as NSString,
    ]
    return HostProviderConfigurations(transparent: transparent, dnsBootstrap: bootstrap)
}

private func captureSnapshot(
    from request: HostEnableRequest
) throws -> CaptureConfigurationSnapshot {
    var rules: [CaptureRule] = []
    for (index, rule) in request.processRules.enumerated() {
        let sources = sourceMatchers(from: rule.pattern)
        guard !sources.isEmpty else { continue }
        rules.append(
            try CaptureRule(
                id: "process-\(index)",
                enabled: true,
                priority: (index + 1) * 10,
                sources: sources,
                destinations: [],
                protocols: [],
                portRanges: [],
                action: captureAction(via: rule.via),
                unavailableFallback: .direct
            )
        )
    }
    rules.append(
        try CaptureRule(
            id: "default-profile-rules",
            enabled: true,
            priority: 10_000,
            sources: [],
            destinations: [],
            protocols: [],
            portRanges: [],
            action: .mihomo(.profileRules),
            unavailableFallback: .direct
        )
    )
    return try CaptureConfigurationSnapshot(revision: request.revision, rules: rules)
}

private func sourceMatchers(from raw: String) -> [SourceMatcher] {
    let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !trimmed.isEmpty else { return [] }

    var sources: [SourceMatcher] = []
    if let path = executablePath(from: trimmed) {
        sources.append(.executable(ExecutableSourceMatcher(canonicalPath: path)))
    }

    var seen = Set<String>()
    for candidate in identifierCandidates(trimmed) {
        let key = candidate.lowercased()
        guard seen.insert(key).inserted else { continue }
        guard let pattern = try? ApplicationIdentifierPatternMatcher(pattern: candidate) else {
            continue
        }
        sources.append(.applicationIdentifierPattern(pattern))
    }
    return sources
}

/// Proxifier-style Application values: process name, bundle id, `Foo.app`,
/// or an absolute Mach-O path. A `.app` bundle directory is not an executable.
private func executablePath(from raw: String) -> String? {
    guard raw.hasPrefix("/"), raw.count > 1 else { return nil }
    guard !raw.lowercased().hasSuffix(".app") else { return nil }
    guard !raw.contains(where: { $0 == "\0" || $0 == "\n" || $0 == "\r" }) else { return nil }
    return raw
}

private func identifierCandidates(_ raw: String) -> [String] {
    var values: [String] = []
    func add(_ value: String) {
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        guard !values.contains(where: { $0.caseInsensitiveCompare(trimmed) == .orderedSame }) else {
            return
        }
        values.append(trimmed)
    }

    add(raw)
    if raw.contains("/") {
        let url = URL(fileURLWithPath: raw)
        add(url.lastPathComponent)
        var current = url
        for _ in 0..<8 {
            if current.pathExtension.lowercased() == "app" {
                add(current.deletingPathExtension().lastPathComponent)
                break
            }
            let parent = current.deletingLastPathComponent()
            if parent.path == current.path || parent.path == "/" {
                break
            }
            current = parent
        }
    } else if raw.lowercased().hasSuffix(".app") {
        add(String(raw.dropLast(4)))
    }
    return values
}

private func captureAction(via: String) -> CaptureAction {
    switch via.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() {
    case "direct":
        return .direct
    case "reject":
        return .reject
    default:
        let name = via.trimmingCharacters(in: .whitespacesAndNewlines)
        if name.isEmpty {
            return .mihomo(.profileRules)
        }
        return .mihomo(.group(name))
    }
}

private func routeEndpoints(
    from request: HostEnableRequest
) throws -> [MihomoRouteProxyEndpoint] {
    var endpoints = [
        try MihomoRouteProxyEndpoint(
            route: .profileRules,
            host: "127.0.0.1",
            port: request.socksPort,
            username: request.username,
            password: request.password
        ),
    ]
    var seen = Set<String>()
    for group in request.groupPorts {
        let name = group.name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty, seen.insert(name).inserted else { continue }
        endpoints.append(
            try MihomoRouteProxyEndpoint(
                route: .group(name),
                host: "127.0.0.1",
                port: group.port,
                username: request.username,
                password: request.password
            )
        )
    }
    return endpoints
}

private func duplicateString(_ value: String) -> UnsafeMutablePointer<CChar> {
    let utf8 = Array(value.utf8CString)
    let pointer = UnsafeMutablePointer<CChar>.allocate(capacity: utf8.count)
    pointer.initialize(from: utf8, count: utf8.count)
    return pointer
}

private func runBlocking<T: Sendable>(
    _ operation: @escaping @Sendable () async throws -> T
) throws -> T {
    let semaphore = DispatchSemaphore(value: 0)
    let box = BlockingResult<T>()
    Task {
        do {
            box.set(.success(try await operation()))
        } catch {
            box.set(.failure(error))
        }
        semaphore.signal()
    }
    semaphore.wait()
    return try box.get()
}

private final class BlockingResult<T: Sendable>: @unchecked Sendable {
    private let lock = NSLock()
    private var value: Result<T, Error>?

    func set(_ value: Result<T, Error>) {
        lock.lock()
        self.value = value
        lock.unlock()
    }

    func get() throws -> T {
        lock.lock()
        defer { lock.unlock() }
        guard let value else {
            throw NetworkExtensionControlFailure(
                operation: .configureTransparentProxy,
                message: "Host controller returned no result"
            )
        }
        return try value.get()
    }
}

@_cdecl("myproxy_ne_free_string")
public func myproxy_ne_free_string(_ value: UnsafeMutablePointer<CChar>?) {
    value?.deallocate()
}

@_cdecl("myproxy_ne_enable")
public func myproxy_ne_enable(
    _ json: UnsafePointer<CChar>?,
    _ errorOut: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?
) -> Int32 {
    errorOut?.pointee = nil
    guard let json else {
        errorOut?.pointee = duplicateString("missing Network Extension configuration")
        return -1
    }
    let payload = String(cString: json)
    do {
        let outcome = try runBlocking {
            try await HostController.shared.enable(json: payload)
        }
        switch outcome {
        case .running:
            return 0
        case .requiresReboot:
            return 2
        }
    } catch {
        errorOut?.pointee = duplicateString(error.localizedDescription)
        return -1
    }
}

@_cdecl("myproxy_ne_disable")
public func myproxy_ne_disable(
    _ errorOut: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?
) -> Int32 {
    errorOut?.pointee = nil
    do {
        try runBlocking {
            try await HostController.shared.disable()
        }
        return 0
    } catch {
        errorOut?.pointee = duplicateString(error.localizedDescription)
        return -1
    }
}
