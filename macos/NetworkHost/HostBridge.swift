import Darwin
import Foundation
import MyproxyNetworkShared

private struct HostEnableRequest: Decodable, Sendable {
    struct ProcessRule: Decodable, Sendable {
        let order: UInt64
        let pattern: String
        let via: String
        let protocols: [TransportProtocol]?
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
        let protocols: [TransportProtocol]?
    }

    struct QualifiedRule: Decodable, Sendable {
        let order: UInt64
        let via: String
        let userIds: [UInt32]
        let ports: [UInt16]
        let destinations: [DestRule]
        let protocols: [TransportProtocol]
    }

    let revision: UInt64
    let operationRevision: UInt64
    let socksPort: UInt16
    let username: String
    let password: String
    let appAdmission: AppAdmissionBootstrap?
    let processRules: [ProcessRule]
    let destRules: [DestRule]
    let qualifiedRules: [QualifiedRule]?
    let gfwDomains: [String]
    let groupPorts: [GroupPort]
    let gfwPorts: [GroupPort]
    /// Upstream resolvers for system lookups macOS reports as a name endpoint.
    let dnsResolvers: [String]?
    let capturePrivateNetworks: Bool?
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
        #if MYPROXY_XRAY
        let environmentKey = "MYPROXY_XRAY_DATA_DIR"
        let directoryName = "myproxy-xray"
        #else
        let environmentKey = "MYPROXY_DATA_DIR"
        let directoryName = "myproxy"
        #endif
        if let override = ProcessInfo.processInfo.environment[environmentKey], !override.isEmpty {
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
        return root.appendingPathComponent(directoryName, isDirectory: true)
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
        // A process stuck in `myproxy_ne_wait` can remain UE-exiting and keep
        // the old flock forever. A new file lets a recovered host proceed.
        let path = try HostSharedIntent.stateURL("network-extension-side-effects.lock").path
        let descriptor = open(path, O_RDWR | O_CREAT | O_CLOEXEC, 0o600)
        guard descriptor >= 0 else {
            throw NetworkExtensionControlFailure(
                operation: .configureTransparentProxy, message: "无法打开系统接管操作锁"
            )
        }
        do {
            let clock = ContinuousClock()
            let deadline = clock.now.advanced(by: .seconds(45))
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
                if clock.now >= deadline {
                    throw NetworkExtensionControlFailure(
                        operation: .configureTransparentProxy,
                        message: "等待系统接管操作锁超时"
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
    var captureEnabled = false
    var failOpen = true
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
            await Self.awaitSuperseded(previous)
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

    /// Wait for a cancelled predecessor, but do not inherit its hang.
    /// `TaskGroup.cancelAll` cannot interrupt `await previous.value`, so this
    /// uses a once-resolve race: predecessor completion or the supersede
    /// deadline, whichever finishes first.
    private static func awaitSuperseded(_ previous: Task<Void, Never>?) async {
        guard let previous else { return }
        let reply = OnceReply<Void>()
        do {
            try await withCheckedThrowingContinuation {
                (continuation: CheckedContinuation<Void, Error>) in
                reply.attach(continuation)
                Task {
                    await previous.value
                    reply.finish(.success(()))
                }
                let parts = OnceReplyTimeout.supersededOperation.components
                let seconds = TimeInterval(parts.seconds)
                    + TimeInterval(parts.attoseconds) / 1_000_000_000_000_000_000
                DispatchQueue.global().asyncAfter(deadline: .now() + seconds) {
                    reply.finish(
                        .failure(
                            NetworkExtensionControlFailure(
                                operation: .configureTransparentProxy,
                                message: "superseded host operation deadline"
                            )
                        )
                    )
                }
            }
        } catch {
            AppLog.warn(
                "ne-host",
                "superseded host operation still running after cancel; continuing"
            )
        }
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
        var applied = false
        #if !MYPROXY_XRAY
        let connected = await transparentProxy.isConnected()
        try intent.check()
        let canLiveUpdate = connected
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
                try intent.check()
                applied = true
            } catch {
                try intent.check()
                AppLog.warn("ne-host", "live capture update failed; restarting provider")
            }
        }
        #endif
        if !applied {
            try await disableDNSProxyAllowingDenied(intent: intent)
            try intent.check()
            try await transparentProxy.stop(dropWedgedConfiguration: true)
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
            // Preferences and the SOCKS backend can both look ready while
            // getaddrinfo is still hung on a name-endpoint flow.
            let probe = await dnsProxy.proveSystemResolver()
            AppLog.info("ne-host", "system resolver probe=\(probe)")
            switch probe {
            case .intercepted, .clear:
                break
            case .unproven:
                throw NetworkExtensionControlFailure(
                    operation: .configureDNSProxy,
                    message: "system resolver did not return after NEDNSProxy enable"
                )
            }
        } catch {
            try intent.check()
            HostRuntime.shared.update(operation: operation) {
                $0.dnsPhase = "failed"
                $0.dnsMessage = error.localizedDescription
            }
            dnsConfigurationError = (intent, error.localizedDescription)
            #if MYPROXY_XRAY
            try await disableDNSProxyAllowingDenied(intent: intent)
            try intent.check()
            try await transparentProxy.stop(dropWedgedConfiguration: true)
            try intent.check()
            lastEndpoints = []
            lastSocksPort = nil
            lastUsername = nil
            lastPassword = nil
            HostRuntime.shared.update(operation: operation) {
                $0.phase = "failed"
                $0.captureEnabled = false
                $0.failOpen = true
                $0.appliedRevision = nil
                $0.message = "DNS 启动失败，已关闭系统接管。本地 HTTP 和 SOCKS5 代理仍可使用。"
            }
            return
            #else
            // A half-enabled NEDNSProxy with no backend blackholes getaddrinfo.
            try? await disableDNSProxyAllowingDenied(intent: intent)
            #endif
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
            $0.captureEnabled = false
            $0.failOpen = true
        }
    }

    func activityBatch(cursor: UInt64, limit: Int) async throws -> HostActivityProcessBatch {
        let response = try await transparentProxy.fetchActivity(cursor: cursor, limit: limit)
        return HostActivityProcessBatch(response.activityBatch)
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
                $0.captureEnabled = response.captureEnabled
                $0.failOpen = response.failOpen ?? true
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
            let detail = describeProviderStatusError(error)
            AppLog.error("ne-host", "refreshStatus failed: \(detail)")
            HostRuntime.shared.update(operation: operation, observation: observation) {
                guard ["running", "disabled", "failed"].contains($0.phase) else { return }
                $0.phase = "failed"
                $0.message = "无法读取系统接管 Provider 状态：\(detail)"
                $0.dnsPhase = "unknown"
            }
        }
    }
}

private func describeProviderStatusError(_ error: Error) -> String {
    if let failure = error as? NetworkExtensionControlFailure {
        return failure.message
    }
    let nsError = error as NSError
    if nsError.domain == NSCocoaErrorDomain {
        return error.localizedDescription
    }
    return "\(nsError.domain) \(nsError.code) — \(error.localizedDescription)"
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

/// Resolvers the DNS provider may relay name-endpoint queries to. Entries the
/// provider cannot dial as a plain address (a hostname or a DoH URL) are
/// dropped rather than failing activation, because they never reach a resolver.
/// An empty remainder uses the same defaults as the provider data plane.
private func usableResolvers(from specs: [String]?) -> [String] {
    let usable = (specs ?? []).filter(DNSProxyUpstreamResolver.isValid)
    if let specs, usable.count != specs.count {
        AppLog.warn(
            "ne-host",
            "dns resolvers dropped=\(specs.count - usable.count) because they are not plain addresses"
        )
    }
    return DNSProxyUpstreamResolver.resolved(usable.isEmpty ? nil : usable)
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
    #if MYPROXY_XRAY
    let admission = try applicationAdmission(from: request)
    let bootstrap = try DNSProxyBootstrapConfiguration(
        revision: request.revision,
        activationIdentifier: activationIdentifier,
        profileRulesProxy: endpoints[0],
        upstreamResolvers: usableResolvers(from: request.dnsResolvers),
        appAdmission: admission
    ).encoded()
    let transparent: [String: NSObject] = [
        "revision": NSNumber(value: request.revision),
        "activationIdentifier": activationIdentifier.uuidString as NSString,
        "dnsProxyBootstrap": bootstrap as NSData,
        "captureEnabled": NSNumber(value: true),
        "failOpen": NSNumber(value: false),
        "appAdmission": try JSONEncoder().encode(admission) as NSData,
        "mihomoSOCKSHost": "127.0.0.1" as NSString,
        "mihomoSOCKSPort": NSNumber(value: request.socksPort),
        "mihomoSOCKSUsername": request.username as NSString,
        "mihomoSOCKSPassword": request.password as NSString,
    ]
    return HostProviderConfigurations(transparent: transparent, dnsBootstrap: bootstrap)
    #else
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
        upstreamResolvers: usableResolvers(from: request.dnsResolvers),
        encodedCaptureSnapshot: encodedSnapshot
    ).encoded()
    let transparent: [String: NSObject] = [
        "revision": NSNumber(value: request.revision),
        "activationIdentifier": activationIdentifier.uuidString as NSString,
        "dnsProxyBootstrap": bootstrap as NSData,
        "captureEnabled": NSNumber(value: true),
        "failOpen": NSNumber(value: captureFailureOpensDirectly),
        "captureConfigurationSnapshot": encodedSnapshot as NSData,
        "mihomoRouteProxyCatalog": catalog as NSData,
        "mihomoSOCKSHost": "127.0.0.1" as NSString,
        "mihomoSOCKSPort": NSNumber(value: request.socksPort),
        "mihomoSOCKSUsername": request.username as NSString,
        "mihomoSOCKSPassword": request.password as NSString,
    ]
    return HostProviderConfigurations(transparent: transparent, dnsBootstrap: bootstrap)
    #endif
}

private func applicationAdmission(from request: HostEnableRequest) throws -> AppAdmissionBootstrap {
    guard let admission = request.appAdmission,
          admission.version == 1, admission.port > 0,
          UUID(uuidString: admission.activation) != nil,
          Data(base64Encoded: admission.key)?.count == 32,
          request.processRules.isEmpty, request.destRules.isEmpty,
          (request.qualifiedRules ?? []).isEmpty, request.groupPorts.isEmpty,
          request.gfwPorts.isEmpty, request.gfwDomains.isEmpty else {
        throw NetworkExtensionControlFailure(operation: .configureTransparentProxy,
            message: "系统接管需要应用决策服务，不能包含路由规则")
    }
    return admission
}

private func validateHostRequest(_ request: HostEnableRequest) throws {
    #if MYPROXY_XRAY
    _ = try applicationAdmission(from: request)
    #else
    _ = try captureSnapshot(from: request)
    #endif
    _ = try routeEndpoints(from: request)
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
        protocols: [TransportProtocol] = [],
        portRanges: [PortRange] = [],
        via: String
    ) throws {
        rules.append(try CaptureRule(
            id: id,
            priority: rules.count,
            sources: sources,
            destinations: destinations,
            protocols: Set(protocols),
            portRanges: portRanges,
            action: captureAction(via: via),
            unavailableFallback: captureFallback(via: via)
        ))
    }

    enum OrderedInput {
        case process(Int, HostEnableRequest.ProcessRule)
        case destination(Int, HostEnableRequest.DestRule)
        case qualified(Int, HostEnableRequest.QualifiedRule)
        var order: UInt64 {
            switch self {
            case .process(_, let rule): rule.order
            case .destination(_, let rule): rule.order
            case .qualified(_, let rule): rule.order
            }
        }
    }
    let inputs = request.processRules.enumerated().map { OrderedInput.process($0.offset, $0.element) }
        + request.destRules.enumerated().map { OrderedInput.destination($0.offset, $0.element) }
        + (request.qualifiedRules ?? []).enumerated().map { OrderedInput.qualified($0.offset, $0.element) }
    let ordered = inputs.enumerated().sorted {
        if $0.element.order == $1.element.order { return $0.offset < $1.offset }
        return $0.element.order < $1.element.order
    }
    for input in ordered.map(\.element) {
        switch input {
        case .qualified(let index, let rule):
            let destinations = rule.destinations.flatMap { destinationMatchers(kind: $0.kind, value: $0.value) }
            guard rule.destinations.isEmpty || !destinations.isEmpty else {
                throw NetworkExtensionControlFailure(operation: .configureTransparentProxy, message: "无效的组合目标条件")
            }
            try append("qualified-\(index)", sources: rule.userIds.map { .userID($0) }, destinations: destinations,
                protocols: rule.protocols, portRanges: try rule.ports.map { try PortRange($0) }, via: rule.via)
        case .process(let index, let rule):
            let sources = sourceMatchers(from: rule.pattern)
            guard !sources.isEmpty else {
                throw NetworkExtensionControlFailure(
                    operation: .configureTransparentProxy, message: "无效的应用匹配条件"
                )
            }
            try append("process-\(index)", sources: sources, protocols: rule.protocols ?? [], via: rule.via)
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
            try append("dest-\(index)", destinations: destinations, protocols: rule.protocols ?? [], via: rule.via)
        }
    }
    rules.append(try CaptureRule(
        id: "default-profile-rules",
        priority: rules.count,
        action: .mihomo(.profileRules),
        unavailableFallback: captureFailureOpensDirectly ? .direct : .reject
    ))
    return try CaptureConfigurationSnapshot(
        revision: request.revision,
        rules: rules,
        capturePrivateNetworks: request.capturePrivateNetworks ?? false
    )
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
    case "wildcard":
        return (try? DestinationMatcher.hostPattern(HostPatternMatcher(pattern: trimmed)))
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

private var captureFailureOpensDirectly: Bool {
    #if MYPROXY_XRAY
    return false
    #else
    return true
    #endif
}

private func captureFallback(via: String) -> UnavailableFallback {
    #if MYPROXY_XRAY
    return via.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() == "direct" ? .direct : .reject
    #else
    switch via.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() {
    case "direct":
        return .direct
    case "reject":
        return .reject
    default:
        return .profileRules
    }
    #endif
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
        try validateHostRequest(request)
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
        try validateHostRequest(request)
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

private struct HostActivityProcessBatch: Encodable, Sendable {
    struct Entry: Encodable, Sendable {
        let relayLocalPort: UInt16
        let process: String
        let matcher: String
        let relayState: String
    }

    let entries: [Entry]
    let nextCursor: UInt64
    let hasMore: Bool

    init(_ batch: AppRoutingActivityBatch?) {
        let joinable: Set<AppRoutingRelayState> = [
            .connecting, .ready, .relaying, .completed
        ]
        var entries: [Entry] = []
        if let batch {
            for activity in batch.activities {
                guard let port = activity.relayLocalPort, port > 0 else { continue }
                guard joinable.contains(activity.relayState) else { continue }
                let source = activity.source
                let process = HostActivityProcessBatch.processLabel(source)
                guard process != "—" else { continue }
                entries.append(Entry(
                    relayLocalPort: port,
                    process: process,
                    matcher: HostActivityProcessBatch.matcherLabel(source, fallback: process),
                    relayState: activity.relayState.rawValue
                ))
            }
        }
        self.entries = entries
        self.nextCursor = batch?.nextCursor ?? 0
        self.hasMore = batch?.hasMore ?? false
    }

    private static func processLabel(_ source: AppRoutingActivitySource) -> String {
        if let path = source.executablePath {
            let name = URL(fileURLWithPath: path).lastPathComponent
            if !name.isEmpty { return name }
        }
        if let bundle = source.bundleIdentifier, !bundle.isEmpty { return bundle }
        if let signing = source.signingIdentifier, !signing.isEmpty { return signing }
        return "—"
    }

    private static func matcherLabel(
        _ source: AppRoutingActivitySource,
        fallback: String
    ) -> String {
        if let bundle = source.bundleIdentifier, !bundle.isEmpty { return bundle }
        if let signing = source.signingIdentifier, !signing.isEmpty { return signing }
        return fallback
    }
}

@_cdecl("myproxy_ne_activity_batch")
public func myproxy_ne_activity_batch(
    _ cursor: UInt64,
    _ limit: UInt32
) -> UnsafeMutablePointer<CChar>? {
    let boxed = ActivityBatchBox()
    let semaphore = DispatchSemaphore(value: 0)
    Task {
        do {
            boxed.value = try await HostController.shared.activityBatch(
                cursor: cursor,
                limit: max(1, min(Int(limit), 500))
            )
        } catch {
            boxed.value = HostActivityProcessBatch(nil)
        }
        semaphore.signal()
    }
    _ = semaphore.wait(timeout: .now() + 0.8)
    guard let batch = boxed.value,
          let data = try? JSONEncoder().encode(batch),
          let json = String(data: data, encoding: .utf8) else {
        return duplicateString("{\"entries\":[],\"nextCursor\":0,\"hasMore\":false}")
    }
    return duplicateString(json)
}

private final class ActivityBatchBox: @unchecked Sendable {
    var value: HostActivityProcessBatch?
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

/// Residual FFI wait used by older callers. One source or the deadline, never
/// `run(until:)`, which can sit inside a blocked NE XPC source and starve the
/// Rust disable deadline. Host status polling no longer calls this.
@_cdecl("myproxy_ne_wait")
public func myproxy_ne_wait(_ milliseconds: UInt32) {
    let timeout = TimeInterval(min(max(milliseconds, 1), 100)) / 1000
    _ = RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(timeout))
}
