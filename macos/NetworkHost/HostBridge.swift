import Darwin
import Foundation
import MyproxyNetworkShared

private struct HostEnableRequest: Decodable, Sendable {
    struct ProcessRule: Decodable, Sendable {
        let order: UInt64
        let pattern: String
        let via: String
    }

    struct GroupPort: Decodable, Sendable {
        let name: String
        let port: UInt16
    }

    struct DestRule: Decodable, Sendable {
        let order: UInt64
        let kind: String
        let value: String
        let via: String
    }

    let revision: UInt64
    let operationRevision: UInt64
    let socksPort: UInt16
    let username: String
    let password: String
    let processRules: [ProcessRule]
    let destRules: [DestRule]
    let gfwDomains: [String]
    let groupPorts: [GroupPort]
    let gfwPorts: [GroupPort]
}

/// Shared by the GUI and installed CLI. Atomic replacement defines the latest
/// intent; credentials and strategy contents never enter this file.
private struct HostSharedIntent: Sendable {
    let url: URL
    let token: Data

    static func issue() throws -> HostSharedIntent {
        let url = try stateURL("network-extension.intent")
        try FileManager.default.createDirectory(
            at: url.deletingLastPathComponent(), withIntermediateDirectories: true
        )
        let token = Data(UUID().uuidString.utf8)
        try token.write(to: url, options: .atomic)
        return HostSharedIntent(url: url, token: token)
    }

    var isCurrent: Bool {
        (try? Data(contentsOf: url)) == token
    }

    func check() throws {
        try Task.checkCancellation()
        guard try Data(contentsOf: url) == token else { throw CancellationError() }
    }

    static func stateURL(_ name: String) throws -> URL {
        if let override = ProcessInfo.processInfo.environment["MYPROXY_DATA_DIR"], !override.isEmpty {
            return URL(fileURLWithPath: override, isDirectory: true).appendingPathComponent(name)
        }
        guard let root = FileManager.default.urls(
            for: .applicationSupportDirectory, in: .userDomainMask
        ).first else {
            throw NetworkExtensionControlFailure(
                operation: .configureTransparentProxy,
                message: "无法定位系统接管共享状态目录"
            )
        }
        return root.appendingPathComponent("myproxy", isDirectory: true)
            .appendingPathComponent(name)
    }
}

/// Only actual preference/provider mutations are serialized across processes.
/// Human authorization waits happen before acquiring this lock. A newer intent
/// is published before waiting, so older work stops at its next await boundary.
private final class HostSideEffectLock: @unchecked Sendable {
    private let descriptor: Int32

    private init(descriptor: Int32) { self.descriptor = descriptor }

    static func acquire(for intent: HostSharedIntent) async throws -> HostSideEffectLock {
        let path = try HostSharedIntent.stateURL("network-extension-operation.lock").path
        let descriptor = open(path, O_RDWR | O_CREAT | O_CLOEXEC, 0o600)
        guard descriptor >= 0 else {
            throw NetworkExtensionControlFailure(
                operation: .configureTransparentProxy, message: "无法打开系统接管操作锁"
            )
        }
        do {
            while true {
                try intent.check()
                if flock(descriptor, LOCK_EX | LOCK_NB) == 0 {
                    try intent.check()
                    return HostSideEffectLock(descriptor: descriptor)
                }
                guard errno == EWOULDBLOCK || errno == EINTR else {
                    throw NetworkExtensionControlFailure(
                        operation: .configureTransparentProxy, message: "无法取得系统接管操作锁"
                    )
                }
                try await Task.sleep(for: .milliseconds(100))
            }
        } catch {
            close(descriptor)
            throw error
        }
    }

    deinit {
        flock(descriptor, LOCK_UN)
        close(descriptor)
    }
}

private struct HostRuntimeSnapshot: Encodable, Sendable {
    var phase = "disabled"
    var dnsPhase = "unknown"
    var observed = false
    var desiredRevision: UInt64 = 0
    var appliedRevision: UInt64?
    var message: String?
    var dnsMessage: String?
}

/// A synchronous, privacy-safe FFI snapshot. Network queries run separately;
/// reading status never blocks the GPUI thread or starts a provider.
private final class HostRuntime: @unchecked Sendable {
    static let shared = HostRuntime()
    private let lock = NSLock()
    private var operationRevision: UInt64 = 0
    private var observation = UUID()
    private var value = HostRuntimeSnapshot()
    private var lastRefresh = Date.distantPast
    private var refreshing = false

    func begin(operation: UInt64, desired: UInt64?, phase: String) {
        lock.lock()
        defer { lock.unlock() }
        operationRevision = operation
        observation = UUID()
        value.phase = phase
        value.observed = true
        value.message = nil
        value.dnsMessage = nil
        value.dnsPhase = phase == "stopping" ? "stopping" : "waiting"
        if let desired { value.desiredRevision = desired }
    }

    func update(
        operation: UInt64, observation expectedObservation: UUID? = nil,
        _ change: (inout HostRuntimeSnapshot) -> Void
    ) {
        lock.lock()
        defer { lock.unlock() }
        guard operationRevision == operation else { return }
        if let expectedObservation, observation != expectedObservation { return }
        change(&value)
    }

    func observation(for operation: UInt64) -> UUID? {
        lock.lock()
        defer { lock.unlock() }
        return operationRevision == operation ? observation : nil
    }

    func snapshot() -> HostRuntimeSnapshot {
        lock.lock()
        defer { lock.unlock() }
        return value
    }

    func requestRefresh() -> (operation: UInt64, observation: UUID)? {
        lock.lock()
        defer { lock.unlock() }
        guard !refreshing, Date().timeIntervalSince(lastRefresh) >= 2,
              ["running", "disabled", "failed"].contains(value.phase) else { return nil }
        refreshing = true
        lastRefresh = Date()
        return (operationRevision, observation)
    }

    func finishRefresh(operation: UInt64, observation expectedObservation: UUID) {
        lock.lock()
        refreshing = false
        if operationRevision == operation && observation == expectedObservation { value.observed = true }
        lock.unlock()
    }

    func invalidate(operation: UInt64) {
        lock.lock()
        defer { lock.unlock() }
        guard operationRevision == operation else { return }
        operationRevision = 0
        observation = UUID()
        value.phase = "disabled"
        value.dnsPhase = "unknown"
        value.observed = false
        value.appliedRevision = nil
        value.desiredRevision = 0
        value.message = "其他入口更新了接管配置，正在读取运行状态"
        value.dnsMessage = nil
        lastRefresh = .distantPast
    }
}

/// Serialize side effects, but cancel approval/connection waits immediately.
/// Each step checks cancellation; a late authorization callback cannot restart
/// an operation superseded by disable or another configuration.
private final class HostOperations: @unchecked Sendable {
    static let shared = HostOperations()
    private let lock = NSLock()
    private var task: Task<Void, Never>?
    private var latestRevision: UInt64 = 0
    private var intent: HostSharedIntent?

    func submit(
        revision: UInt64,
        desired: UInt64?,
        phase: String,
        operation: @escaping @Sendable (HostSharedIntent) async throws -> Void
    ) throws -> Bool {
        lock.lock()
        defer { lock.unlock() }
        guard revision > latestRevision else { return false }
        let intent = try HostSharedIntent.issue()
        self.intent = intent
        latestRevision = revision
        let previous = task
        previous?.cancel()
        HostRuntime.shared.begin(operation: revision, desired: desired, phase: phase)
        task = Task {
            await previous?.value
            do {
                try intent.check()
                try await operation(intent)
            } catch is CancellationError {
                // A newer queued operation owns cleanup and the visible state.
            } catch {
                HostRuntime.shared.update(operation: revision) {
                    $0.phase = "failed"
                    $0.message = error.localizedDescription
                }
                AppLog.error("ne-host", "operation failed: \(error.localizedDescription)")
            }
        }
        return true
    }

    func synchronizeIntent() {
        lock.lock()
        defer { lock.unlock() }
        guard let intent, !intent.isCurrent else { return }
        task?.cancel()
        self.intent = nil
        HostRuntime.shared.invalidate(operation: latestRevision)
    }
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
    private var dnsConfigurationError: (intent: HostSharedIntent, message: String)?

    func enable(_ request: HostEnableRequest, intent: HostSharedIntent) async throws {
        let operation = request.operationRevision
        dnsConfigurationError = nil
        let endpoints = try routeEndpoints(from: request)
        let activationIdentifier = UUID()
        let configurations = try providerConfigurations(
            from: request,
            endpoints: endpoints,
            activationIdentifier: activationIdentifier
        )
        try intent.check()
        let outcome = try await systemExtension.activate {
            guard intent.isCurrent else { return }
            HostRuntime.shared.update(operation: operation) { $0.phase = "waitingApproval" }
        }
        try intent.check()
        if case .requiresReboot = outcome {
            HostRuntime.shared.update(operation: operation) {
                $0.phase = "requiresReboot"
                $0.dnsPhase = "unknown"
            }
            return
        }
        let sideEffects = try await HostSideEffectLock.acquire(for: intent)
        defer { withExtendedLifetime(sideEffects) {} }
        try intent.check()
        HostRuntime.shared.update(operation: operation) { $0.phase = "requesting" }
        let connected = await transparentProxy.isConnected()
        try intent.check()
        let canLiveUpdate = connected
            && lastSocksPort == request.socksPort
            && lastUsername == request.username
            && lastPassword == request.password
            && preservesRouteEndpoints(lastEndpoints, endpoints)
        var applied = false
        if canLiveUpdate {
            do {
                try await transparentProxy.configureAndApplyRunning(
                    configurations.transparent,
                    revision: request.revision
                )
                try intent.check()
                applied = true
            } catch {
                try intent.check()
                AppLog.warn("ne-host", "live capture update failed; restarting provider")
            }
        }
        if !applied {
            try await disableDNSProxyAllowingDenied(intent: intent)
            try intent.check()
            try await transparentProxy.stop()
            try intent.check()
            try await transparentProxy.configure(configurations.transparent)
            try intent.check()
            try await transparentProxy.start()
            try intent.check()
        }
        lastEndpoints = endpoints
        lastSocksPort = request.socksPort
        lastUsername = request.username
        lastPassword = request.password
        HostRuntime.shared.update(operation: operation) {
            $0.phase = "requesting"
            $0.appliedRevision = request.revision
            $0.dnsPhase = "waiting"
        }
        do {
            try await transparentProxy.prepareDNS(
                revision: request.revision,
                activationIdentifier: activationIdentifier,
                bootstrap: configurations.dnsBootstrap
            )
            try intent.check()
            try await dnsProxy.configureAndEnable(configurations.dnsBootstrap)
            try intent.check()
            // Saving DNS preferences is not a runtime acknowledgement.
        } catch {
            try intent.check()
            HostRuntime.shared.update(operation: operation) {
                $0.dnsPhase = "failed"
                $0.dnsMessage = error.localizedDescription
            }
            dnsConfigurationError = (intent, error.localizedDescription)
            // A half-enabled NEDNSProxy with no backend blackholes getaddrinfo.
            try? await disableDNSProxyAllowingDenied(intent: intent)
        }
        HostRuntime.shared.update(operation: operation) { $0.phase = "running" }
        if let observation = HostRuntime.shared.observation(for: operation) {
            await refreshStatus(operation: operation, observation: observation)
        }
    }

    func disable(operation: UInt64, intent: HostSharedIntent) async throws {
        let sideEffects = try await HostSideEffectLock.acquire(for: intent)
        defer { withExtendedLifetime(sideEffects) {} }
        try intent.check()
        var firstError: Error?
        // Do not swallow NEDNSProxyErrorDomain 1 here. A false "disabled"
        // lets disconnect kill :1053 while queries are still intercepted.
        do { try await dnsProxy.disable() } catch { firstError = error }
        try intent.check()
        do { try await transparentProxy.stop() } catch { if firstError == nil { firstError = error } }
        try intent.check()
        lastEndpoints = []
        lastSocksPort = nil
        lastUsername = nil
        lastPassword = nil
        dnsConfigurationError = nil
        if let firstError { throw firstError }
        HostRuntime.shared.update(operation: operation) {
            $0.phase = "disabled"
            $0.dnsPhase = "disabled"
            $0.appliedRevision = nil
        }
    }

    func refreshStatus(operation: UInt64, observation: UUID) async {
        do {
            let connected = try await transparentProxy.connectionStatus()
            guard connected else {
                HostRuntime.shared.update(operation: operation, observation: observation) {
                    if $0.phase == "running" {
                        $0.phase = "failed"
                        $0.message = "系统接管连接已断开"
                    }
                    // A disconnected transparent provider cannot prove whether
                    // an independently managed DNS provider is running.
                    if $0.dnsPhase != "disabled" { $0.dnsPhase = "unknown" }
                }
                return
            }
            let response = try await transparentProxy.runtimeStatus()
            HostRuntime.shared.update(operation: operation, observation: observation) {
                guard ["running", "disabled", "failed"].contains($0.phase) else { return }
                guard response.running && response.captureEnabled else {
                    $0.phase = "failed"
                    $0.message = "系统接管 Provider 尚未就绪"
                    $0.dnsPhase = "unknown"
                    return
                }
                let externalChange = $0.appliedRevision != nil && $0.appliedRevision != response.revision
                $0.phase = "running"
                $0.message = externalChange ? "运行配置已由其他入口更新" : nil
                $0.appliedRevision = response.revision
                if $0.desiredRevision == 0 { $0.desiredRevision = response.revision }
                if let dnsConfigurationError, dnsConfigurationError.intent.isCurrent {
                    $0.dnsPhase = "failed"
                    $0.dnsMessage = dnsConfigurationError.message
                    return
                }
                guard let report = response.dnsRuntimeReport else {
                    $0.dnsPhase = "waiting"
                    return
                }
                let revision = response.revision
                let activation = report.expectedActivationIdentifier
                guard report.expectedRevision == revision else {
                    $0.dnsPhase = "unknown"
                    $0.dnsMessage = "DNS 运行报告属于旧配置"
                    return
                }
                if let failure = report.startupFailure {
                    $0.dnsPhase = "failed"
                    $0.dnsMessage = "DNS 启动失败：\(failure.reason.rawValue)"
                    return
                }
                guard let status = report.status else {
                    $0.dnsPhase = "waiting"
                    return
                }
                do {
                    try status.validate(expectedRevision: revision, activationIdentifier: activation)
                    switch status.phase {
                    case .running:
                        $0.dnsPhase = status.backendReady ? "running" : "waiting"
                    case .starting: $0.dnsPhase = "waiting"
                    case .stopping: $0.dnsPhase = "stopping"
                    case .stopped: $0.dnsPhase = "disabled"
                    case .failed: $0.dnsPhase = "failed"
                    }
                    $0.dnsMessage = status.failureCategory?.rawValue
                } catch {
                    $0.dnsPhase = "unknown"
                    $0.dnsMessage = "DNS 运行报告已过期或与当前配置不符"
                }
            }
        } catch {
            HostRuntime.shared.update(operation: operation, observation: observation) {
                guard ["running", "disabled", "failed"].contains($0.phase) else { return }
                $0.phase = "failed"
                $0.message = "无法读取系统接管 Provider 状态"
                $0.dnsPhase = "unknown"
            }
        }
    }
}

private func isRecoverableDNSProxyDisableError(_ error: Error) -> Bool {
    let text = error.localizedDescription
    return text.contains("NEDNSProxyErrorDomain 1") || text.contains("permission denied")
}

private extension HostController {
    /// Only for enable/restart: a denied disable must not block writing a new
    /// DNS configuration. A full disable still throws so the core stays up.
    func disableDNSProxyAllowingDenied(intent: HostSharedIntent) async throws {
        do {
            try await dnsProxy.disable()
        } catch {
            try intent.check()
            guard isRecoverableDNSProxyDisableError(error) else { throw error }
            AppLog.warn(
                "ne-host",
                "dns proxy disable denied; continuing so a new configuration can be saved"
            )
        }
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
    guard request.gfwDomains.isEmpty else {
        throw NetworkExtensionControlFailure(
            operation: .configureTransparentProxy,
            message: "系统接管不再嵌入 GFWList 域名"
        )
    }

    var rules: [CaptureRule] = []

    func append(
        _ id: String,
        sources: [SourceMatcher] = [],
        destinations: [DestinationMatcher] = [],
        via: String
    ) throws {
        rules.append(try CaptureRule(
            id: id,
            priority: rules.count,
            sources: sources,
            destinations: destinations,
            action: captureAction(via: via),
            unavailableFallback: captureFallback(via: via)
        ))
    }

    enum OrderedInput {
        case process(Int, HostEnableRequest.ProcessRule)
        case destination(Int, HostEnableRequest.DestRule)
        var order: UInt64 {
            switch self {
            case .process(_, let rule): rule.order
            case .destination(_, let rule): rule.order
            }
        }
    }
    let inputs = request.processRules.enumerated().map { OrderedInput.process($0.offset, $0.element) }
        + request.destRules.enumerated().map { OrderedInput.destination($0.offset, $0.element) }
    let ordered = inputs.enumerated().sorted {
        if $0.element.order == $1.element.order { return $0.offset < $1.offset }
        return $0.element.order < $1.element.order
    }
    for input in ordered.map(\.element) {
        switch input {
        case .process(let index, let rule):
            let sources = sourceMatchers(from: rule.pattern)
            guard !sources.isEmpty else {
                throw NetworkExtensionControlFailure(
                    operation: .configureTransparentProxy, message: "无效的应用匹配条件"
                )
            }
            try append("process-\(index)", sources: sources, via: rule.via)
        case .destination(let index, let rule):
            if rule.kind == "cidr" && gfwGroup(via: rule.via) != nil {
                throw NetworkExtensionControlFailure(
                    operation: .configureTransparentProxy,
                    message: "GFWList 无法与网段规则求交"
                )
            }
            let destinations = destinationMatchers(kind: rule.kind, value: rule.value)
            guard !destinations.isEmpty else {
                throw NetworkExtensionControlFailure(
                    operation: .configureTransparentProxy, message: "无效的目标匹配条件"
                )
            }
            try append("dest-\(index)", destinations: destinations, via: rule.via)
        }
    }
    rules.append(try CaptureRule(
        id: "default-profile-rules",
        priority: rules.count,
        action: .mihomo(.profileRules),
        unavailableFallback: .direct
    ))
    return try CaptureConfigurationSnapshot(revision: request.revision, rules: rules)
}

private func gfwGroup(via: String) -> String? {
    let trimmed = via.trimmingCharacters(in: .whitespacesAndNewlines)
    let lower = trimmed.lowercased()
    for prefix in ["gfw:", "gfwlist:"] where lower.hasPrefix(prefix) {
        let group = trimmed.dropFirst(prefix.count)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return group.isEmpty ? nil : group
    }
    return nil
}

private func destinationMatchers(kind: String, value: String) -> [DestinationMatcher] {
    let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !trimmed.isEmpty else { return [] }
    switch kind {
    case "suffix":
        let host = trimmed.trimmingCharacters(in: CharacterSet(charactersIn: "."))
        return (try? DestinationMatcher.host(HostMatcher(kind: .suffix, value: host))).map { [$0] } ?? []
    case "domain":
        if trimmed.hasPrefix("*.") {
            let host = String(trimmed.dropFirst(2))
            return (try? DestinationMatcher.host(HostMatcher(kind: .suffix, value: host))).map { [$0] } ?? []
        }
        return (try? DestinationMatcher.host(HostMatcher(kind: .exact, value: trimmed))).map { [$0] } ?? []
    case "keyword":
        return (try? DestinationMatcher.hostPattern(HostPatternMatcher(pattern: "*\(trimmed)*")))
            .map { [$0] } ?? []
    case "cidr":
        return (try? DestinationMatcher.network(IPNetwork(trimmed))).map { [$0] } ?? []
    default:
        return []
    }
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

private func captureFallback(via: String) -> UnavailableFallback {
    switch via.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() {
    case "direct":
        return .direct
    case "reject":
        return .reject
    default:
        return .profileRules
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
    for group in request.groupPorts + request.gfwPorts {
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

@_cdecl("myproxy_ne_free_string")
public func myproxy_ne_free_string(_ value: UnsafeMutablePointer<CChar>?) {
    value?.deallocate()
}

@_cdecl("myproxy_ne_validate")
public func myproxy_ne_validate(
    _ json: UnsafePointer<CChar>?,
    _ errorOut: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?
) -> Int32 {
    errorOut?.pointee = nil
    guard let json else {
        errorOut?.pointee = duplicateString("missing Network Extension configuration")
        return -1
    }
    do {
        let request = try JSONDecoder().decode(
            HostEnableRequest.self, from: Data(String(cString: json).utf8)
        )
        _ = try captureSnapshot(from: request)
        _ = try routeEndpoints(from: request)
        return 0
    } catch {
        errorOut?.pointee = duplicateString(error.localizedDescription)
        return -1
    }
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
    do {
        let request = try JSONDecoder().decode(
            HostEnableRequest.self, from: Data(String(cString: json).utf8)
        )
        // Validate before scheduling side effects or cancelling a working plan.
        _ = try captureSnapshot(from: request)
        _ = try routeEndpoints(from: request)
        let submitted = try HostOperations.shared.submit(
            revision: request.operationRevision,
            desired: request.revision,
            phase: "requesting"
        ) { intent in
            try await HostController.shared.enable(request, intent: intent)
        }
        if !submitted {
            errorOut?.pointee = duplicateString("系统接管操作已被更新请求取代")
            return -1
        }
        return 1
    } catch {
        errorOut?.pointee = duplicateString(error.localizedDescription)
        return -1
    }
}

@_cdecl("myproxy_ne_disable")
public func myproxy_ne_disable(
    _ operationRevision: UInt64,
    _ errorOut: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?
) -> Int32 {
    errorOut?.pointee = nil
    do {
        let submitted = try HostOperations.shared.submit(
            revision: operationRevision, desired: nil, phase: "stopping"
        ) { intent in
            try await HostController.shared.disable(operation: operationRevision, intent: intent)
        }
        if !submitted {
            errorOut?.pointee = duplicateString("系统接管操作已被更新请求取代")
            return -1
        }
        return 1
    } catch {
        errorOut?.pointee = duplicateString(error.localizedDescription)
        return -1
    }
}

@_cdecl("myproxy_ne_status")
public func myproxy_ne_status() -> UnsafeMutablePointer<CChar>? {
    HostOperations.shared.synchronizeIntent()
    if let refresh = HostRuntime.shared.requestRefresh() {
        Task {
            await HostController.shared.refreshStatus(
                operation: refresh.operation, observation: refresh.observation
            )
            HostRuntime.shared.finishRefresh(
                operation: refresh.operation, observation: refresh.observation
            )
        }
    }
    guard let data = try? JSONEncoder().encode(HostRuntime.shared.snapshot()),
          let json = String(data: data, encoding: .utf8) else { return nil }
    return duplicateString(json)
}

/// CLI waits service the native run loop so preference/authorization callbacks
/// can complete before the command exits. GUI reads use myproxy_ne_status only.
@_cdecl("myproxy_ne_wait")
public func myproxy_ne_wait(_ milliseconds: UInt32) {
    let deadline = Date().addingTimeInterval(Double(min(milliseconds, 100)) / 1000)
    RunLoop.current.run(until: deadline)
    let remaining = deadline.timeIntervalSinceNow
    if remaining > 0 { Thread.sleep(forTimeInterval: remaining) }
}
