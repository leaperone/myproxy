import Foundation
import MyproxyNetworkShared
@preconcurrency import Network
@preconcurrency import NetworkExtension
import OSLog

private let dnsProxyProviderLogger = Logger(
    subsystem: "local.harry.myproxy.network-extension",
    category: "DNSProxyProvider"
)

enum DNSProxyBootstrapError: LocalizedError {
    case dataPlaneUnavailable
    case cancelledDuringStartup
    case missingProviderConfiguration
    case missingBootstrapPayload
    case invalidBootstrapPayload
    case invalidPrivateRelay

    var errorDescription: String? {
        switch self {
        case .dataPlaneUnavailable:
            return "The private Mihomo SOCKS5 DNS relay is unavailable"
        case .cancelledDuringStartup:
            return "DNS proxy startup was cancelled before the private relay became ready"
        case .missingProviderConfiguration:
            return "The DNS proxy provider configuration was not delivered by macOS"
        case .missingBootstrapPayload:
            return "The DNS proxy configuration is missing its versioned bootstrap payload"
        case .invalidBootstrapPayload:
            return "The DNS proxy bootstrap payload is invalid or unsupported"
        case .invalidPrivateRelay:
            return "The DNS proxy bootstrap contains an invalid private Mihomo SOCKS5 relay"
        }
    }
}

enum DNSRelayRoute: Equatable, Sendable {
    case directTrustedComponent
    case directLocalResolver
    case mihomo(MihomoRoute)

    var bypassesMihomo: Bool {
        switch self {
        case .directTrustedComponent, .directLocalResolver:
            true
        case .mihomo:
            false
        }
    }
}

enum DNSRelayRoutingPolicy {
    static func route(
        destination: SOCKS5Endpoint,
        isTrustedMyproxyComponent: Bool
    ) -> DNSRelayRoute {
        if isTrustedMyproxyComponent {
            return .directTrustedComponent
        }
        if destination.address.ipAddress?.isLocalNetwork == true
            || isLocalResolverDomain(destination.address.domain) {
            return .directLocalResolver
        }
        return .mihomo(.profileRules)
    }

    private static func isLocalResolverDomain(_ domain: String?) -> Bool {
        guard let domain else { return false }
        let normalized = domain
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .lowercased()
            .trimmingCharacters(in: CharacterSet(charactersIn: "."))
        return normalized == "localhost"
            || normalized == "local"
            || normalized == "localdomain"
            || normalized.hasSuffix(".local")
            || normalized.hasSuffix(".lan")
            || normalized.hasSuffix(".localdomain")
            || normalized.hasSuffix(".home.arpa")
    }
}

/// DNS proxy entry point. Public DNS endpoints are relayed through the same
/// private authenticated Mihomo SOCKS5 listener used by App Routing. Local
/// resolvers are relayed directly so requests such as `192.168.1.1:53` do not
/// make a redundant round trip through Mihomo or pollute its connection list.
/// A relay failure is reported to the host heartbeat so the host can disable
/// the DNS manager and restore the system resolver.
final class DNSProxyProvider: NEDNSProxyProvider, @unchecked Sendable {
    private let runtime = ProviderRuntimeState(providerName: "dns-proxy")
    private let flowDecisionCoordinator = NetworkExtensionFlowDecisionCoordinator()
    private let tcpRelays = TCPFlowRelayRegistry()
    private let udpSessions = UDPFlowSessionRegistry()
    private var reporter: DNSProxyRuntimeReporter?
    private var proxy: ProviderSOCKSConfiguration?
    private var proxyCatalog: [MihomoRoute: ProviderSOCKSConfiguration] = [:]
    private let backendProbeQueue = DispatchQueue(
        label: "local.harry.myproxy.dns-backend-probe"
    )
    private let backendProbeLock = NSLock()
    private var backendProbeTimer: DispatchSourceTimer?
    private var backendProbeConfirmationTimer: DispatchSourceTimer?
    private var backendProbeConfirmationID: UUID?
    private var activeBackendProbe: MihomoUDPAssociationProbe?
    private var pendingStartCompletion: DNSProxyLifecycleCompletion?
    private var liveUpdaterToken: UUID?
    private var backendProbeGeneration: UInt64 = 0
    private var consecutiveBackendProbeFailures = 0
    private var backendProbingSuspended = false

    private static let backendProbeFailureThreshold = 3
    private static let backendProbeConfirmationDelay: DispatchTimeInterval = .seconds(4)

    override func startProxy(
        options: [String: Any]? = nil,
        completionHandler: @escaping (Error?) -> Void
    ) {
        let startCompletion = DNSProxyLifecycleCompletion(completionHandler)
        backendProbeLock.lock()
        let staleConfirmationTimer = takeBackendProbeConfirmationTimerLocked()
        staleConfirmationTimer?.cancel()
        backendProbeGeneration &+= 1
        let startGeneration = backendProbeGeneration
        backendProbingSuspended = true
        pendingStartCompletion = startCompletion

        let providerConfiguration = options
        let deliveredPayload = providerConfiguration.flatMap {
            Self.data($0[ProviderConfigurationKey.dnsProxyBootstrap])
        }
        let deliveredBootstrap = deliveredPayload.flatMap {
            try? DNSProxyBootstrapConfiguration.decode($0)
        }
        let bootstrap: DNSProxyBootstrapConfiguration
        do {
            bootstrap = try DNSProxyRuntimeRegistry.shared.resolveBootstrap(
                delivered: deliveredBootstrap
            )
        } catch {
            let reason: DNSProxyStartupFailureReason
            let bootstrapError: DNSProxyBootstrapError
            if options == nil {
                reason = .missingProviderConfiguration
                bootstrapError = .missingProviderConfiguration
            } else if deliveredPayload == nil {
                reason = .missingBootstrapPayload
                bootstrapError = .missingBootstrapPayload
            } else {
                reason = .invalidBootstrapPayload
                bootstrapError = .invalidBootstrapPayload
            }
            rejectBootstrapLocked(
                reason: reason,
                error: bootstrapError,
                bootstrap: nil,
                startCompletion: startCompletion
            )
            return
        }
        guard let dataPlane = Self.dataPlaneConfiguration(for: bootstrap) else {
            rejectBootstrapLocked(
                reason: .invalidPrivateRelay,
                error: .invalidPrivateRelay,
                bootstrap: bootstrap,
                startCompletion: startCompletion
            )
            return
        }
        flowDecisionCoordinator.load(configuration: dataPlane.routingConfiguration)

        do {
            let reporter = try DNSProxyRuntimeReporter(
                revision: bootstrap.revision,
                activationIdentifier: bootstrap.activationIdentifier
            )
            let probe = MihomoUDPAssociationProbe()
            backendProbingSuspended = false
            self.reporter = reporter
            self.proxy = dataPlane.proxy
            self.proxyCatalog = dataPlane.proxyCatalog
            consecutiveBackendProbeFailures = 0
            activeBackendProbe = probe
            pendingStartCompletion = startCompletion
            reporter.resumeHeartbeat()
            runtime.start(configuration: options)
            liveUpdaterToken = DNSProxyRuntimeRegistry.shared.registerLiveUpdater {
                [weak self] bootstrap in
                self?.applyLiveBootstrap(bootstrap) ?? false
            }
            backendProbeLock.unlock()
            dnsProxyProviderLogger.notice(
                "Accepted DNS bootstrap revision=\(bootstrap.revision, privacy: .public) schema=\(bootstrap.schemaVersion, privacy: .public) source=\(deliveredBootstrap == nil ? "provider-registry" : "provider-options", privacy: .public) payloadBytes=\(deliveredPayload?.count ?? 0, privacy: .public)"
            )
            startBackendStartupProbe(
                probe,
                proxy: dataPlane.proxy,
                startCompletion: startCompletion,
                generation: startGeneration
            )
        } catch {
            pendingStartCompletion = nil
            let runtime = runtime
            backendProbeQueue.async {
                runtime.stop()
                dnsProxyProviderLogger.error(
                    "DNS runtime reporter startup failed errorType=\(String(describing: type(of: error)), privacy: .public)"
                )
                startCompletion.call(error)
            }
            backendProbeLock.unlock()
        }
    }

    private func startBackendStartupProbe(
        _ probe: MihomoUDPAssociationProbe,
        proxy: ProviderSOCKSConfiguration,
        startCompletion: DNSProxyLifecycleCompletion,
        generation: UInt64
    ) {
        probe.start(proxy: proxy) { [weak self] error in
            guard let self else { return }
            self.backendProbeLock.lock()
            let resultIsCurrent = self.activeBackendProbe === probe
                && self.backendProbeGeneration == generation
                && !self.backendProbingSuspended
            guard resultIsCurrent,
                  self.pendingStartCompletion === startCompletion,
                  let currentReporter = self.reporter else {
                self.backendProbeLock.unlock()
                return
            }
            if let error {
                self.activeBackendProbe = nil
                self.pendingStartCompletion = nil
                self.reporter = nil
                self.proxy = nil
                self.proxyCatalog = [:]
                self.backendProbingSuspended = true
                self.backendProbeGeneration &+= 1
                let runtime = self.runtime
                self.backendProbeQueue.async {
                    currentReporter.markStartupFailed(.backendUnavailable)
                    runtime.stop()
                    startCompletion.call(error)
                }
                self.backendProbeLock.unlock()
                return
            }
            do {
                try currentReporter.markRunning()
                self.activeBackendProbe = nil
                self.pendingStartCompletion = nil
                self.backendProbeQueue.async { [weak self] in
                    self?.startPeriodicBackendProbe()
                    startCompletion.call(nil)
                }
                self.backendProbeLock.unlock()
            } catch {
                self.activeBackendProbe = nil
                self.pendingStartCompletion = nil
                self.reporter = nil
                self.proxy = nil
                self.proxyCatalog = [:]
                self.backendProbingSuspended = true
                self.backendProbeGeneration &+= 1
                let runtime = self.runtime
                self.backendProbeQueue.async {
                    currentReporter.markStartupFailed(.statusPersistenceFailed)
                    runtime.stop()
                    startCompletion.call(error)
                }
                self.backendProbeLock.unlock()
            }
        }
    }

    /// Called only while `backendProbeLock` owns the startup lifecycle slot.
    private func rejectBootstrapLocked(
        reason: DNSProxyStartupFailureReason,
        error: DNSProxyBootstrapError,
        bootstrap: DNSProxyBootstrapConfiguration?,
        startCompletion: DNSProxyLifecycleCompletion
    ) {
        pendingStartCompletion = nil
        let runtime = runtime
        backendProbeQueue.async {
            if let bootstrap {
                DNSProxyRuntimeRegistry.shared.publishStartupFailure(
                    reason,
                    for: bootstrap
                )
            }
            runtime.start(configuration: nil)
            runtime.stop()
            dnsProxyProviderLogger.error(
                "Rejected DNS bootstrap reason=\(reason.rawValue, privacy: .public)"
            )
            startCompletion.call(error)
        }
        backendProbeLock.unlock()
    }

    override func stopProxy(
        with reason: NEProviderStopReason,
        completionHandler: @escaping () -> Void
    ) {
        let stopCompletion = DNSProxyLifecycleCompletion { _ in
            completionHandler()
        }
        backendProbeLock.lock()
        backendProbingSuspended = true
        backendProbeGeneration &+= 1
        consecutiveBackendProbeFailures = 0
        let timer = backendProbeTimer
        backendProbeTimer = nil
        let confirmationTimer = takeBackendProbeConfirmationTimerLocked()
        let probe = activeBackendProbe
        activeBackendProbe = nil
        let pendingStartCompletion = pendingStartCompletion
        self.pendingStartCompletion = nil
        let reporter = reporter
        self.reporter = nil
        let liveUpdaterToken = liveUpdaterToken
        self.liveUpdaterToken = nil
        proxy = nil
        proxyCatalog = [:]
        flowDecisionCoordinator.quiesce()
        if let liveUpdaterToken {
            DNSProxyRuntimeRegistry.shared.unregisterLiveUpdater(liveUpdaterToken)
        }
        backendProbeLock.unlock()
        timer?.setEventHandler {}
        timer?.cancel()
        cancelBackendProbeConfirmationTimer(confirmationTimer)
        probe?.cancel()
        backendProbeQueue.async { [self] in
            pendingStartCompletion?.call(DNSProxyBootstrapError.cancelledDuringStartup)
            tcpRelays.cancelAll()
            udpSessions.cancelAll()
            reporter?.stop(category: .cancelled)
            runtime.stop()
            stopCompletion.call(nil)
        }
    }

    override func handleNewFlow(_ flow: NEAppProxyFlow) -> Bool {
        guard let tcpFlow = flow as? NEAppProxyTCPFlow else { return true }
        let runtimeState = runtimeDataPlaneSnapshot()
        guard runtimeState.proxy != nil,
              let destination = DNSProxyEndpointCompatibility.tcpDestination(tcpFlow)
        else {
            reject(flow, category: .flowConversionFailed)
            return true
        }
        let identifier = UUID()
        runtimeState.reporter?.beginFlow(identifier, transportProtocol: .tcp)
        let reporter = runtimeState.reporter
        let observer: @Sendable (AppRoutingRelaySnapshot) -> Void = { snapshot in
            reporter?.observe(
                snapshot,
                flowIdentifier: identifier,
                transportProtocol: .tcp
            )
        }
        let sourceIsTrusted = flowDecisionCoordinator.isTrustedMyproxyComponent(tcpFlow)
        let baseRoute = DNSRelayRoutingPolicy.route(
            destination: destination,
            isTrustedMyproxyComponent: sourceIsTrusted
        )
        let route = resolvedMihomoRoute(
            baseRoute,
            flow: tcpFlow,
            destination: destination,
            transportProtocol: .tcp,
            proxyCatalog: runtimeState.proxyCatalog
        )
        if route.bypassesMihomo {
            // Mihomo's own DNS egress must not be sent back through Mihomo's
            // SOCKS listener. Relay it from the provider process, whose own
            // sockets are outside the DNS interception path, to break the
            // otherwise recursive DNS→SOCKS→DNS loop.
            tcpRelays.startDirect(
                flow: tcpFlow,
                destination: destination,
                relayNote: directRelayNote(for: route),
                activityObserver: observer
            )
        } else {
            guard case let .mihomo(mihomoRoute) = route,
                  let proxy = runtimeState.proxyCatalog[mihomoRoute] else {
                reject(flow, category: .backendUnavailable)
                return true
            }
            tcpRelays.startMihomo(
                flow: tcpFlow,
                proxy: proxy,
                destination: destination,
                unavailableFallback: .reject,
                activityObserver: observer
            )
        }
        return true
    }

    @available(macOS, introduced: 14.0, obsoleted: 15.0)
    override func __handleNewUDPFlow(
        _ flow: NEAppProxyUDPFlow,
        initialRemoteEndpoint remoteEndpoint: NetworkExtension.__NWEndpoint
    ) -> Bool {
        guard let destination = DNSProxyEndpointCompatibility.legacyDestination(
            remoteEndpoint
        ) else {
            reject(flow, category: .flowConversionFailed)
            return true
        }
        return handleUDPFlow(flow, initialDestination: destination)
    }

    override func sleep(completionHandler: @escaping () -> Void) {
        suspendBackendProbing()
        runtimeDataPlaneSnapshot().reporter?.pauseHeartbeat()
        tcpRelays.cancelAll()
        udpSessions.cancelAll()
        completionHandler()
    }

    override func wake() {
        backendProbeLock.lock()
        guard backendProbingSuspended, let reporter, let proxy else {
            backendProbeLock.unlock()
            return
        }
        backendProbingSuspended = false
        consecutiveBackendProbeFailures = 0
        backendProbeGeneration &+= 1
        let generation = backendProbeGeneration
        let startCompletion = pendingStartCompletion
        let startupProbe = startCompletion.map { _ in MihomoUDPAssociationProbe() }
        if let startupProbe {
            activeBackendProbe = startupProbe
        }
        backendProbeLock.unlock()
        reporter.resumeHeartbeat()
        if let startCompletion, let startupProbe {
            startBackendStartupProbe(
                startupProbe,
                proxy: proxy,
                startCompletion: startCompletion,
                generation: generation
            )
            return
        }
        runBackendProbe()
        startPeriodicBackendProbe()
    }

    private func startPeriodicBackendProbe() {
        backendProbeLock.lock()
        guard backendProbeTimer == nil, !backendProbingSuspended else {
            backendProbeLock.unlock()
            return
        }
        let timer = DispatchSource.makeTimerSource(queue: backendProbeQueue)
        let generation = backendProbeGeneration
        timer.schedule(
            deadline: .now() + .seconds(60),
            repeating: .seconds(60),
            leeway: .seconds(10)
        )
        timer.setEventHandler { [weak self] in
            self?.runBackendProbe(expectedGeneration: generation)
        }
        timer.resume()
        backendProbeTimer = timer
        backendProbeLock.unlock()
    }

    private func suspendBackendProbing() {
        backendProbeLock.lock()
        backendProbingSuspended = true
        backendProbeGeneration &+= 1
        consecutiveBackendProbeFailures = 0
        let timer = backendProbeTimer
        backendProbeTimer = nil
        let confirmationTimer = takeBackendProbeConfirmationTimerLocked()
        let probe = activeBackendProbe
        activeBackendProbe = nil
        backendProbeLock.unlock()
        timer?.setEventHandler {}
        timer?.cancel()
        cancelBackendProbeConfirmationTimer(confirmationTimer)
        probe?.cancel()
    }

    private func runBackendProbe(expectedGeneration: UInt64? = nil) {
        backendProbeLock.lock()
        guard expectedGeneration == nil || expectedGeneration == backendProbeGeneration,
              activeBackendProbe == nil,
              !backendProbingSuspended,
              let proxy,
              let reporter
        else {
            backendProbeLock.unlock()
            return
        }
        let probe = MihomoUDPAssociationProbe()
        let probeGeneration = backendProbeGeneration
        activeBackendProbe = probe
        backendProbeLock.unlock()
        probe.start(proxy: proxy) { [weak self] error in
            guard let self else { return }
            self.backendProbeLock.lock()
            let resultIsCurrent = self.activeBackendProbe === probe
                && self.backendProbeGeneration == probeGeneration
                && !self.backendProbingSuspended
            guard resultIsCurrent else {
                self.backendProbeLock.unlock()
                return
            }
            self.activeBackendProbe = nil
            if error == nil {
                self.consecutiveBackendProbeFailures = 0
            } else {
                self.consecutiveBackendProbeFailures += 1
            }
            let stableFailure = self.consecutiveBackendProbeFailures
                >= Self.backendProbeFailureThreshold
            let needsConfirmation = error != nil && !stableFailure
            let currentReporter = self.reporter === reporter ? reporter : nil
            self.backendProbeLock.unlock()

            if error == nil {
                try? currentReporter?.markRunning()
            } else if stableFailure {
                currentReporter?.markBackendUnavailable(.backendUnavailable)
            }
            if needsConfirmation {
                self.scheduleBackendProbeConfirmation(generation: probeGeneration)
            }
        }
    }

    private func scheduleBackendProbeConfirmation(generation: UInt64) {
        backendProbeLock.lock()
        guard generation == backendProbeGeneration,
              !backendProbingSuspended,
              backendProbeConfirmationTimer == nil else {
            backendProbeLock.unlock()
            return
        }
        let confirmationID = UUID()
        let timer = DispatchSource.makeTimerSource(queue: backendProbeQueue)
        timer.schedule(
            deadline: .now() + Self.backendProbeConfirmationDelay,
            leeway: .seconds(1)
        )
        timer.setEventHandler { [weak self] in
            self?.runBackendProbeConfirmation(
                id: confirmationID,
                generation: generation
            )
        }
        backendProbeConfirmationID = confirmationID
        backendProbeConfirmationTimer = timer
        timer.resume()
        backendProbeLock.unlock()
    }

    private func runBackendProbeConfirmation(id: UUID, generation: UInt64) {
        backendProbeLock.lock()
        guard backendProbeConfirmationID == id else {
            backendProbeLock.unlock()
            return
        }
        let timer = takeBackendProbeConfirmationTimerLocked()
        guard backendProbeGeneration == generation,
              !backendProbingSuspended else {
            backendProbeLock.unlock()
            cancelBackendProbeConfirmationTimer(timer)
            return
        }
        backendProbeLock.unlock()
        cancelBackendProbeConfirmationTimer(timer)
        runBackendProbe(expectedGeneration: generation)
    }

    private func takeBackendProbeConfirmationTimerLocked() -> DispatchSourceTimer? {
        let timer = backendProbeConfirmationTimer
        backendProbeConfirmationTimer = nil
        backendProbeConfirmationID = nil
        return timer
    }

    private func cancelBackendProbeConfirmationTimer(_ timer: DispatchSourceTimer?) {
        timer?.setEventHandler {}
        timer?.cancel()
    }

    private func handleUDPFlow(
        _ flow: NEAppProxyUDPFlow,
        initialDestination: SOCKS5Endpoint
    ) -> Bool {
        let runtimeState = runtimeDataPlaneSnapshot()
        guard runtimeState.proxy != nil else {
            reject(flow, category: .backendUnavailable)
            return true
        }
        let parentIdentifier = UUID()
        let sourceIsTrusted = flowDecisionCoordinator.isTrustedMyproxyComponent(flow)
        let initialRoute = resolvedMihomoRoute(
            DNSRelayRoutingPolicy.route(
                destination: initialDestination,
                isTrustedMyproxyComponent: sourceIsTrusted
            ),
            flow: flow,
            destination: initialDestination,
            transportProtocol: .udp,
            proxyCatalog: runtimeState.proxyCatalog
        )
        let initialProxy = proxy(
            for: initialRoute,
            in: runtimeState.proxyCatalog
        )
        let initialPlan = dnsPlan(
            destination: initialDestination,
            proxy: initialProxy,
            route: initialRoute,
            parentIdentifier: parentIdentifier
        )
        let reporter = runtimeState.reporter
        let started = udpSessions.start(
            id: parentIdentifier,
            flow: flow,
            initialPlan: initialPlan,
            planner: { [weak self] destination in
                guard let self else {
                    return initialPlan
                }
                let currentState = self.runtimeDataPlaneSnapshot()
                guard currentState.proxy != nil else { return initialPlan }
                let route = self.resolvedMihomoRoute(
                    DNSRelayRoutingPolicy.route(
                        destination: destination,
                        isTrustedMyproxyComponent: sourceIsTrusted
                    ),
                    flow: flow,
                    destination: destination,
                    transportProtocol: .udp,
                    proxyCatalog: currentState.proxyCatalog
                )
                return self.dnsPlan(
                    destination: destination,
                    proxy: self.proxy(for: route, in: currentState.proxyCatalog),
                    route: route,
                    parentIdentifier: parentIdentifier
                )
            },
            revisionProvider: { initialPlan.activity.configurationRevision },
            activitySink: { activity in
                reporter?.beginFlow(
                    activity.flowIdentifier,
                    transportProtocol: .udp
                )
            },
            observerFactory: { identifier in
                { snapshot in
                    reporter?.observe(
                        snapshot,
                        flowIdentifier: identifier,
                        transportProtocol: .udp
                    )
                }
            }
        )
        if !started {
            reject(flow, category: .udpRelayFailed)
        }
        return true
    }

    private func dnsPlan(
        destination: SOCKS5Endpoint,
        proxy: ProviderSOCKSConfiguration?,
        route: DNSRelayRoute,
        parentIdentifier: UUID
    ) -> UDPFlowInterceptionPlan {
        let bypassMihomo = route.bypassesMihomo
        let mihomoRoute: MihomoRoute = {
            guard case let .mihomo(value) = route else { return .profileRules }
            return value
        }()
        let decision = FlowTrafficDecision(
            disposition: bypassMihomo ? .direct : .mihomo(mihomoRoute),
            reason: route == .directTrustedComponent
                ? .rule(.builtInBypass(.trustedmyproxyComponent))
                : .rule(.defaultDirect)
        )
        let host = destination.address.ipAddress?.presentation
            ?? destination.address.domain
        let activity = AppRoutingActivity(
            parentFlowIdentifier: parentIdentifier,
            captureOrigin: .dnsProxy,
            configurationRevision: runtime.snapshot().revision,
            startedAt: Date(),
            source: AppRoutingActivitySource(
                processIdentifier: 0,
                userIdentifier: 0,
                signingIdentifier: "System DNS"
            ),
            destination: AppRoutingActivityDestination(
                hostname: destination.address.domain,
                ipAddress: destination.address.ipAddress?.presentation ?? host,
                port: destination.port
            ),
            transportProtocol: .udp,
            decision: decision,
            configuredAction: bypassMihomo ? .direct : .mihomo(mihomoRoute),
            effectiveAction: bypassMihomo ? .direct : .mihomo(mihomoRoute),
            relayState: .pending,
            payloadBytesAreMeasured: true,
            uploadDatagrams: 0,
            downloadDatagrams: 0,
            droppedDatagrams: 0
        )
        return UDPFlowInterceptionPlan(
            decision: decision,
            initialDestination: destination,
            mihomoDestination: destination,
            proxy: proxy,
            unavailableFallback: .reject,
            activity: activity,
            parentFlowIdentifier: parentIdentifier
        )
    }

    private func directRelayNote(for route: DNSRelayRoute) -> String {
        switch route {
        case .directTrustedComponent:
            "Trusted myproxy DNS egress bypassed the private SOCKS listener."
        case .directLocalResolver:
            "Local DNS resolver bypassed the private SOCKS listener."
        case let .mihomo(route):
            "DNS relayed through \(route.stableSortKey)."
        }
    }

    private func resolvedMihomoRoute(
        _ baseRoute: DNSRelayRoute,
        flow: NEAppProxyFlow,
        destination: SOCKS5Endpoint,
        transportProtocol: TransportProtocol,
        proxyCatalog: [MihomoRoute: ProviderSOCKSConfiguration]
    ) -> DNSRelayRoute {
        guard !baseRoute.bypassesMihomo else { return baseRoute }
        let decision = flowDecisionCoordinator.decideDNSFlow(
            flow,
            destination: destination,
            transportProtocol: transportProtocol
        )
        if case let .mihomo(route) = decision.disposition,
           proxyCatalog[route] != nil {
            return .mihomo(route)
        }
        // Destination-only rules cannot be evaluated from a DNS resolver
        // flow, and some system-generated DNS flows have no usable app
        // identity. Both cases deliberately retain the primary profile route.
        return .mihomo(.profileRules)
    }

    private func proxy(
        for route: DNSRelayRoute,
        in proxyCatalog: [MihomoRoute: ProviderSOCKSConfiguration]
    ) -> ProviderSOCKSConfiguration? {
        guard case let .mihomo(mihomoRoute) = route else { return nil }
        return proxyCatalog[mihomoRoute]
    }

    private func reject(
        _ flow: NEAppProxyFlow,
        category: DNSProxyFailureCategory
    ) {
        let reporter = runtimeDataPlaneSnapshot().reporter
        let identifier = UUID()
        let transportProtocol: TransportProtocol = flow is NEAppProxyUDPFlow
            ? .udp
            : .tcp
        reporter?.beginFlow(identifier, transportProtocol: transportProtocol)
        reporter?.observe(
            AppRoutingRelaySnapshot(
                state: .failed,
                uploadBytes: 0,
                downloadBytes: 0,
                error: "DNS relay unavailable",
                localPort: nil
            ),
            flowIdentifier: identifier,
            transportProtocol: transportProtocol
        )
        reporter?.recordFlowFailure(category)
        let error = DNSProxyBootstrapError.dataPlaneUnavailable
        AppProxyFlowCompatibility.open(flow) { completionError in
            let terminalError = completionError ?? error
            flow.closeReadWithError(terminalError)
            flow.closeWriteWithError(terminalError)
        }
    }

    private func runtimeDataPlaneSnapshot() -> (
        reporter: DNSProxyRuntimeReporter?,
        proxy: ProviderSOCKSConfiguration?,
        proxyCatalog: [MihomoRoute: ProviderSOCKSConfiguration]
    ) {
        backendProbeLock.lock()
        let snapshot = (reporter, proxy, proxyCatalog)
        backendProbeLock.unlock()
        return snapshot
    }

    private func applyLiveBootstrap(
        _ bootstrap: DNSProxyBootstrapConfiguration
    ) -> Bool {
        guard let dataPlane = Self.dataPlaneConfiguration(for: bootstrap) else {
            return false
        }
        guard bootstrap.revision > runtime.snapshot().revision else {
            return false
        }
        let newReporter: DNSProxyRuntimeReporter
        do {
            newReporter = try DNSProxyRuntimeReporter(
                revision: bootstrap.revision,
                activationIdentifier: bootstrap.activationIdentifier
            )
            newReporter.resumeHeartbeat()
            try newReporter.markRunning()
        } catch {
            dnsProxyProviderLogger.error(
                "DNS live update reporter failed revision=\(bootstrap.revision, privacy: .public)"
            )
            return false
        }

        backendProbeLock.lock()
        guard reporter != nil, proxy != nil, !backendProbingSuspended else {
            backendProbeLock.unlock()
            newReporter.stop(category: .cancelled)
            return false
        }
        // Keep the decision snapshot and route catalog replacement inside the
        // same provider lifecycle critical section. New flows block while the
        // data plane is replaced; existing relays retain the proxy and
        // reporter captured when they started.
        flowDecisionCoordinator.load(configuration: dataPlane.routingConfiguration)
        backendProbeGeneration &+= 1
        consecutiveBackendProbeFailures = 0
        let previousTimer = backendProbeTimer
        backendProbeTimer = nil
        let previousConfirmationTimer = takeBackendProbeConfirmationTimerLocked()
        let previousReporter = reporter
        let previousProbe = activeBackendProbe
        activeBackendProbe = nil
        reporter = newReporter
        proxy = dataPlane.proxy
        proxyCatalog = dataPlane.proxyCatalog
        runtime.replace(
            revision: bootstrap.revision,
            captureEnabled: true,
            failOpen: true
        )
        backendProbeLock.unlock()

        previousTimer?.setEventHandler {}
        previousTimer?.cancel()
        cancelBackendProbeConfirmationTimer(previousConfirmationTimer)
        previousProbe?.cancel()
        previousReporter?.stop()
        runBackendProbe()
        startPeriodicBackendProbe()
        dnsProxyProviderLogger.notice(
            "DNS rules updated live revision=\(bootstrap.revision, privacy: .public)"
        )
        return true
    }

    private struct DataPlaneConfiguration {
        let proxy: ProviderSOCKSConfiguration
        let proxyCatalog: [MihomoRoute: ProviderSOCKSConfiguration]
        let routingConfiguration: [String: Any]
    }

    private static func dataPlaneConfiguration(
        for bootstrap: DNSProxyBootstrapConfiguration
    ) -> DataPlaneConfiguration? {
        guard let proxy = ProviderSOCKSConfiguration(
            routeEndpoint: bootstrap.profileRulesProxy
        ) else { return nil }
        let routeEndpoints = bootstrap.routeProxyEndpoints
            ?? [bootstrap.profileRulesProxy]
        var proxyCatalog: [MihomoRoute: ProviderSOCKSConfiguration] = [:]
        for endpoint in routeEndpoints {
            guard let configuration = ProviderSOCKSConfiguration(
                routeEndpoint: endpoint
            ) else { return nil }
            proxyCatalog[endpoint.route] = configuration
        }
        guard proxyCatalog[.profileRules] != nil,
              let catalogData = try? MihomoRouteProxyCatalog.encode(routeEndpoints)
        else { return nil }
        var routingConfiguration: [String: Any] = [
            ProviderConfigurationKey.revision: NSNumber(value: bootstrap.revision),
            ProviderConfigurationKey.captureEnabled: NSNumber(value: true),
            ProviderConfigurationKey.mihomoRouteProxyCatalog: catalogData,
        ]
        if let snapshot = bootstrap.encodedCaptureSnapshot {
            routingConfiguration[ProviderConfigurationKey.captureConfigurationSnapshot] =
                snapshot
        }
        return DataPlaneConfiguration(
            proxy: proxy,
            proxyCatalog: proxyCatalog,
            routingConfiguration: routingConfiguration
        )
    }

    private static func data(_ value: Any?) -> Data? {
        switch value {
        case let value as Data: value
        case let value as NSData: value as Data
        default: nil
        }
    }
}

private final class DNSProxyLifecycleCompletion: @unchecked Sendable {
    private let lock = NSLock()
    private let completion: (Error?) -> Void
    private var completed = false

    init(_ completion: @escaping (Error?) -> Void) {
        self.completion = completion
    }

    func call(_ error: Error?) {
        lock.lock()
        guard !completed else {
            lock.unlock()
            return
        }
        completed = true
        lock.unlock()
        completion(error)
    }
}

private enum DNSProxyEndpointCompatibility {
    static func tcpDestination(_ flow: NEAppProxyTCPFlow) -> SOCKS5Endpoint? {
        if #available(macOS 15.0, *) {
            return destination(flow.remoteFlowEndpoint)
        }
        return legacyDestination(flow.__remoteEndpoint)
    }

    @available(macOS 15.0, *)
    static func destination(_ endpoint: Network.NWEndpoint) -> SOCKS5Endpoint? {
        guard case let .hostPort(host, port) = endpoint else { return nil }
        return try? ProviderSOCKSConfiguration.destination(
            for: FlowRemoteEndpoint(host: host.debugDescription, port: port.rawValue)
        )
    }

    @available(macOS, introduced: 14.0, obsoleted: 15.0)
    static func legacyDestination(
        _ endpoint: NetworkExtension.__NWEndpoint
    ) -> SOCKS5Endpoint? {
        let object = endpoint as NSObject
        guard object.isKind(of: NetworkExtension.__NWHostEndpoint.self),
              let host = object.value(forKey: "hostname") as? String,
              let port = object.value(forKey: "port") as? String else { return nil }
        return try? ProviderSOCKSConfiguration.destination(
            for: FlowRemoteEndpoint(host: host, port: port)
        )
    }
}

@available(macOS 15.0, *)
extension DNSProxyProvider: NEAppProxyUDPFlowHandling {
    func handleNewUDPFlow(
        _ flow: NEAppProxyUDPFlow,
        initialRemoteFlowEndpoint remoteEndpoint: Network.NWEndpoint
    ) -> Bool {
        guard let destination = DNSProxyEndpointCompatibility.destination(remoteEndpoint) else {
            reject(flow, category: .flowConversionFailed)
            return true
        }
        return handleUDPFlow(flow, initialDestination: destination)
    }
}
