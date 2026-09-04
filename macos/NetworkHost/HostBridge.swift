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

    func enable(json: String) async throws -> HostEnableOutcome {
        let request = try JSONDecoder().decode(
            HostEnableRequest.self,
            from: Data(json.utf8)
        )
        let configuration = try providerConfiguration(from: request)
        let outcome = try await systemExtension.activate()
        if case .requiresReboot = outcome {
            return .requiresReboot
        }
        try? await transparentProxy.stop()
        try await transparentProxy.configure(configuration)
        try await transparentProxy.start()
        return .running
    }

    func disable() async throws {
        try await transparentProxy.stop()
    }
}

private func providerConfiguration(
    from request: HostEnableRequest
) throws -> [String: NSObject] {
    let snapshot = try captureSnapshot(from: request)
    let encoder = JSONEncoder()
    encoder.dateEncodingStrategy = .iso8601
    let encodedSnapshot = try encoder.encode(snapshot)
    let endpoints = try routeEndpoints(from: request)
    let catalog = try MihomoRouteProxyCatalog.encode(endpoints)
    return [
        "revision": NSNumber(value: request.revision),
        "activationIdentifier": UUID().uuidString as NSString,
        "captureEnabled": NSNumber(value: true),
        "failOpen": NSNumber(value: true),
        "captureConfigurationSnapshot": encodedSnapshot as NSData,
        "mihomoRouteProxyCatalog": catalog as NSData,
        "mihomoSOCKSHost": "127.0.0.1" as NSString,
        "mihomoSOCKSPort": NSNumber(value: request.socksPort),
        "mihomoSOCKSUsername": request.username as NSString,
        "mihomoSOCKSPassword": request.password as NSString,
    ]
}

private func captureSnapshot(
    from request: HostEnableRequest
) throws -> CaptureConfigurationSnapshot {
    var rules: [CaptureRule] = []
    for (index, rule) in request.processRules.enumerated() {
        guard let pattern = try? ApplicationIdentifierPatternMatcher(pattern: rule.pattern) else {
            continue
        }
        rules.append(
            try CaptureRule(
                id: "process-\(index)",
                enabled: true,
                priority: (index + 1) * 10,
                sources: [.applicationIdentifierPattern(pattern)],
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
