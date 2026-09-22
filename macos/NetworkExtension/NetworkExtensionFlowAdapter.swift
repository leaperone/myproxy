import Foundation
import MyproxyNetworkShared
import Network
import NetworkExtension

enum InitialFlowOwnershipPolicy {
    static func shouldEvaluate(metadataSigningIdentifier: String) -> Bool {
        !TrustedMyproxyComponentPolicy().contains(
            metadataSigningIdentifier: metadataSigningIdentifier
        )
    }

    /// Returning `false` from an NE transparent provider preserves the original
    /// application connection. Direct must therefore never be owned merely for
    /// byte accounting; doing so adds a second socket and a user-space relay to
    /// traffic that explicitly requested the native network path.
    static func owns(_ disposition: FlowTrafficDisposition) -> Bool {
        switch disposition {
        case .direct, .failOpen:
            false
        case .reject, .mihomo:
            true
        }
    }
}

enum MihomoRouteAvailabilityPolicy {
    /// Availability is route-specific. Treating one live listener (normally
    /// Profile Rules) as proof that every group/global listener exists can turn
    /// a requested Direct fallback into an unnecessary owned relay.
    static func resolve(
        _ decision: FlowTrafficDecision,
        availableRoutes: Set<MihomoRoute>,
        rulesByIdentifier: [String: CaptureRule]
    ) -> FlowTrafficDecision {
        guard case let .mihomo(route) = decision.disposition,
              !availableRoutes.contains(route),
              case let .rule(cause) = decision.reason else {
            return decision
        }
        let requestedFallback: UnavailableFallback
        if case let .matchedRule(identifier) = cause,
           let rule = rulesByIdentifier[identifier] {
            requestedFallback = rule.unavailableFallback
        } else {
            requestedFallback = .direct
        }
        if requestedFallback != .reject,
           route != .profileRules,
           availableRoutes.contains(.profileRules) {
            return FlowTrafficDecision(
                disposition: .mihomo(.profileRules),
                reason: .mihomoUnavailable(rule: cause, fallback: .profileRules),
                ruleEvidence: decision.ruleEvidence
            )
        }
        let disposition: FlowTrafficDisposition = switch requestedFallback {
        case .direct, .profileRules: .direct
        case .reject: .reject
        }
        return FlowTrafficDecision(
            disposition: disposition,
            reason: .mihomoUnavailable(rule: cause, fallback: requestedFallback),
            ruleEvidence: decision.ruleEvidence
        )
    }
}

enum DNSProfileRoutingRulePolicy {
    /// A DNS proxy flow exposes the resolver endpoint, not the hostname the
    /// source application is resolving. Only a source-scoped rule with no
    /// destination or port constraint may select an explicit Profile here.
    /// Filtering before evaluation prevents a higher-priority resolver-IP or
    /// port-53 rule from shadowing a later application rule.
    static func eligible(_ rule: CaptureRule) -> Bool {
        guard rule.enabled,
              !rule.sources.isEmpty,
              rule.destinations.isEmpty,
              rule.portRanges.isEmpty,
              case let .mihomo(route) = rule.action,
              route.routingProfileID != nil else {
            return false
        }
        return true
    }
}

final class NetworkExtensionFlowDecisionCoordinator: @unchecked Sendable {
    private struct State: Sendable {
        var revision: UInt64 = 0
        var captureEnabled = false
        var preparedConfiguration = PreparedCaptureConfiguration(
            .failOpen(.missingEncodedSnapshot)
        )
        var dnsPreparedConfiguration = PreparedCaptureConfiguration(
            .failOpen(.missingEncodedSnapshot)
        )
        var mihomoSOCKSConfigurations: [MihomoRoute: ProviderSOCKSConfiguration] = [:]
        var availableMihomoRoutes: Set<MihomoRoute> = []
        var rulesByIdentifier: [String: CaptureRule] = [:]
        var dnsRulesByIdentifier: [String: CaptureRule] = [:]
        #if MYPROXY_XRAY
        var admission: AppAdmissionClient?
        var admissionActivation: String = ""
        #endif
    }

    private let lock = NSLock()
    private let identityResolver = ProcessIdentityResolver()
    private let identityCache = ProcessIdentityResolutionCache(capacity: 256)
    private let trustedComponentPolicy = TrustedMyproxyComponentPolicy()
    private let contextBuilder = FlowContextBuilder()
    private let decisionAdapter = FlowTrafficDecisionAdapter()
    private var state = State()

    func load(configuration: [String: Any]?) {
        #if MYPROXY_XRAY
        let admissionData = configuration?[ProviderConfigurationKey.appAdmission] as? Data
        lock.lock()
        state.admission = AppAdmissionClient(data: admissionData)
        state.captureEnabled = state.admission != nil
        state.admissionActivation = state.admission?.activation ?? ""
        state.revision = Self.uint64(configuration?[ProviderConfigurationKey.revision]) ?? 0
        lock.unlock()
        return
        #endif
        let captureEnabled = Self.bool(
            configuration?[ProviderConfigurationKey.captureEnabled]
        ) ?? false
        let encodedSnapshot = configuration?[
            ProviderConfigurationKey.captureConfigurationSnapshot
        ] as? Data
        let loadResult = CaptureConfigurationSnapshotLoader().load(encodedSnapshot)
        // Compile destination indexes once per provider configuration load,
        // never once per intercepted connection.
        let preparedConfiguration = PreparedCaptureConfiguration(loadResult)
        let dnsRules = loadResult.snapshot?.rules.filter(
            DNSProfileRoutingRulePolicy.eligible
        ) ?? []
        let dnsLoadResult: CaptureConfigurationLoadResult
        if let snapshot = loadResult.snapshot,
           let filteredSnapshot = try? CaptureConfigurationSnapshot(
               revision: snapshot.revision,
               generationID: snapshot.generationID,
               createdAt: snapshot.createdAt,
               rules: dnsRules,
               capturePrivateNetworks: snapshot.capturePrivateNetworks
           ) {
            dnsLoadResult = .loaded(filteredSnapshot)
        } else {
            dnsLoadResult = loadResult
        }

        lock.lock()
        state.revision = Self.uint64(configuration?[ProviderConfigurationKey.revision]) ?? 0
        state.captureEnabled = captureEnabled
        state.preparedConfiguration = preparedConfiguration
        state.dnsPreparedConfiguration = PreparedCaptureConfiguration(
            dnsLoadResult
        )
        let routeCatalog = ProviderSOCKSConfiguration.routeCatalog(
            providerConfiguration: configuration
        ) ?? [:]
        state.mihomoSOCKSConfigurations = routeCatalog
        state.availableMihomoRoutes = Set(routeCatalog.keys)
        state.rulesByIdentifier = Dictionary(
            uniqueKeysWithValues: loadResult.snapshot?.rules.map { ($0.id, $0) } ?? []
        )
        state.dnsRulesByIdentifier = Dictionary(
            uniqueKeysWithValues: dnsRules.map { ($0.id, $0) }
        )
        lock.unlock()
    }

    func quiesce() {
        lock.lock()
        state.captureEnabled = false
        lock.unlock()
    }

    func validates(configuration: [String: Any]) -> Bool {
        #if MYPROXY_XRAY
        return AppAdmissionClient(data: configuration[ProviderConfigurationKey.appAdmission] as? Data) != nil
        #endif
        let captureEnabled = Self.bool(
            configuration[ProviderConfigurationKey.captureEnabled]
        ) ?? false
        guard captureEnabled else { return true }
        let snapshot = CaptureConfigurationSnapshotLoader().load(
            configuration[ProviderConfigurationKey.captureConfigurationSnapshot] as? Data
        )
        guard case .loaded = snapshot else { return false }
        return ProviderSOCKSConfiguration.routeCatalog(
            providerConfiguration: configuration
        ) != nil
    }

    func planTCPFlow(_ flow: NEAppProxyTCPFlow) -> TCPFlowInterceptionPlan {
        #if MYPROXY_XRAY
        return planAdmissionTCPFlow(flow)
        #else
        let endpoint: FlowRemoteEndpoint
        if #available(macOS 15.0, *) {
            guard let converted = Self.endpoint(flow.remoteFlowEndpoint) else {
                return TCPFlowInterceptionPlan(
                    decision: failOpen(.unsupportedRemoteEndpoint),
                    destination: nil,
                    mihomoDestination: nil,
                    proxy: nil,
                    unavailableFallback: .direct,
                    activity: fallbackActivity(
                        flow: flow,
                        endpoint: nil,
                        transportProtocol: .tcp,
                        failure: .unsupportedRemoteEndpoint
                    )
                )
            }
            endpoint = converted
        } else {
            guard let converted = Self.legacyEndpoint(flow.__remoteEndpoint) else {
                return TCPFlowInterceptionPlan(
                    decision: failOpen(.unsupportedRemoteEndpoint),
                    destination: nil,
                    mihomoDestination: nil,
                    proxy: nil,
                    unavailableFallback: .direct,
                    activity: fallbackActivity(
                        flow: flow,
                        endpoint: nil,
                        transportProtocol: .tcp,
                        failure: .unsupportedRemoteEndpoint
                    )
                )
            }
            endpoint = converted
        }
        let currentState = snapshotState()
        let outcome = decide(
            flow: flow,
            endpoint: endpoint,
            transportProtocol: .tcp,
            state: currentState
        )
        let routePlan = try? ProviderSOCKSConfiguration.flowPlan(
            for: outcome.decision,
            endpoint: endpoint,
            preferredHostname: outcome.destinationHostname,
            routeCatalog: currentState.mihomoSOCKSConfigurations
        )
        return TCPFlowInterceptionPlan(
            decision: outcome.decision,
            destination: routePlan?.destinations.original,
            mihomoDestination: routePlan.map {
                DNSProxyUpstreamResolver.relayDestination(
                    for: $0.destinations.mihomo,
                    resolvers: [$0.destinations.original]
                )
            },
            proxy: routePlan?.proxy,
            unavailableFallback: unavailableFallbackRequested(
                by: outcome.decision,
                rulesByIdentifier: currentState.rulesByIdentifier
            ),
            activity: outcome.activity
        )
        #endif
    }

    #if MYPROXY_XRAY
    private func planAdmissionTCPFlow(_ flow: NEAppProxyTCPFlow) -> TCPFlowInterceptionPlan {
        guard let endpoint = Self.endpointTCP(flow) else {
            let decision = FlowTrafficDecision(disposition: .reject, reason: .contextUnavailable(.unsupportedRemoteEndpoint))
            return TCPFlowInterceptionPlan(decision: decision, destination: nil, mihomoDestination: nil, proxy: nil, unavailableFallback: .reject, activity: fallbackActivity(flow: flow, endpoint: nil, transportProtocol: .tcp, failure: .unsupportedRemoteEndpoint))
        }
        let result = admissionOutcome(flow: flow, endpoint: endpoint, transport: .tcp, kind: "traffic")
        let target = result.target
        return TCPFlowInterceptionPlan(decision: result.decision, destination: result.decision.disposition == FlowTrafficDisposition.direct ? target : endpointAsSocks(endpoint), mihomoDestination: target, proxy: result.proxy, unavailableFallback: .reject, activity: result.activity)
    }

    private func planAdmissionUDP(_ flow: NEAppProxyUDPFlow, endpoint: FlowRemoteEndpoint, parentFlowIdentifier: UUID, useFlowHostname: Bool = true) -> UDPFlowInterceptionPlan {
        let result = admissionOutcome(flow: flow, endpoint: endpoint, transport: .udp, kind: "traffic", parentFlowIdentifier: parentFlowIdentifier, useFlowHostname: useFlowHostname)
        let target = result.target
        return UDPFlowInterceptionPlan(decision: result.decision, initialDestination: endpointAsSocks(endpoint), mihomoDestination: target, directDestination: target, proxy: result.proxy, unavailableFallback: .reject, activity: result.activity, parentFlowIdentifier: parentFlowIdentifier)
    }

    private struct AdmissionOutcome {
        let decision: FlowTrafficDecision
        let target: SOCKS5Endpoint?
        let proxy: ProviderSOCKSConfiguration?
        let activity: AppRoutingActivity
    }

    private func admissionOutcome(flow: NEAppProxyFlow, endpoint: FlowRemoteEndpoint, transport: TransportProtocol, kind: String, parentFlowIdentifier: UUID? = nil, useFlowHostname: Bool = true) -> AdmissionOutcome {
        let flowID = UUID()
        let currentState = snapshotState()
        guard currentState.captureEnabled else {
            let decision = FlowTrafficDecision(disposition: .direct, reason: .rule(.defaultDirect))
            let original = endpointAsSocks(endpoint)
            let target = kind == "dns" ? original.map { DNSProxyUpstreamResolver.relayDestination(for: $0) } : original
            return AdmissionOutcome(decision: decision, target: target, proxy: nil, activity: fallbackActivity(flow: flow, endpoint: endpoint, transportProtocol: transport, failure: .unsupportedRemoteEndpoint))
        }
        let identityResolution = resolveIdentity(flow)
        let contextResolution = contextBuilder.resolve(endpoint: endpoint, remoteHostname: useFlowHostname ? flow.remoteHostname : nil, metadata: FlowApplicationMetadata(sourceAppAuditToken: flow.metaData.sourceAppAuditToken, sourceAppUniqueIdentifier: flow.metaData.sourceAppUniqueIdentifier, sourceAppSigningIdentifier: flow.metaData.sourceAppSigningIdentifier), identityResolution: identityResolution, transportProtocol: transport, isTrustedMyproxyComponent: trustedComponentPolicy.contains(identityResolution))
        guard let request = makeAdmissionRequest(flow: flow, endpoint: endpoint, transport: transport, kind: kind, flowID: flowID, context: contextResolution, activation: currentState.admissionActivation),
              let client = currentState.admission else {
            let decision = FlowTrafficDecision(disposition: .reject, reason: .configurationUnavailable(.missingEncodedSnapshot))
            return AdmissionOutcome(decision: decision, target: nil, proxy: nil, activity: fallbackActivity(flow: flow, endpoint: endpoint, transportProtocol: transport, failure: .unsupportedRemoteEndpoint))
        }
        let reply: AppAdmissionReply
        switch client.request(request) {
        case let .success(value): reply = value
        case .failure:
            let decision = FlowTrafficDecision(disposition: .reject, reason: .configurationUnavailable(.missingEncodedSnapshot))
            return AdmissionOutcome(decision: decision, target: nil, proxy: nil, activity: fallbackActivity(flow: flow, endpoint: endpoint, transportProtocol: transport, failure: .unsupportedRemoteEndpoint))
        }
        let target = admissionEndpoint(host: reply.host, port: reply.port)
        let decision: FlowTrafficDecision
        let proxy: ProviderSOCKSConfiguration?
        switch reply.action {
        case "direct":
            decision = FlowTrafficDecision(disposition: .direct, reason: .rule(.defaultDirect)); proxy = nil
        case "proxy":
            guard let port = reply.relayPort, let lease = reply.lease, let password = reply.password,
                  (try? SOCKS5UsernamePasswordCredentials(username: lease, password: password)) != nil else {
                let reject = FlowTrafficDecision(disposition: .reject, reason: .configurationUnavailable(.missingEncodedSnapshot))
                return AdmissionOutcome(decision: reject, target: nil, proxy: nil, activity: fallbackActivity(flow: flow, endpoint: endpoint, transportProtocol: transport, failure: .unsupportedRemoteEndpoint))
            }
            decision = FlowTrafficDecision(disposition: .mihomo(.profileRules), reason: .rule(.defaultDirect))
            proxy = try? ProviderSOCKSConfiguration(routeEndpoint: MihomoRouteProxyEndpoint(route: .profileRules, host: "127.0.0.1", port: port, username: lease, password: password))
        default:
            decision = FlowTrafficDecision(disposition: .reject, reason: .rule(.defaultDirect)); proxy = nil
        }
        let activity = makeActivity(flow: flow, endpoint: endpoint, transportProtocol: transport, context: contextResolution, identityResolution: identityResolution, decision: decision, state: snapshotState(), flowIdentifier: flowID, parentFlowIdentifier: parentFlowIdentifier)
        return AdmissionOutcome(decision: decision, target: target, proxy: proxy, activity: activity)
    }

    private func makeAdmissionRequest(flow: NEAppProxyFlow, endpoint: FlowRemoteEndpoint, transport: TransportProtocol, kind: String, flowID: UUID, context: FlowContextResolution, activation: String) -> AppAdmissionRequest? {
        guard let resolved = context.context else { return nil }
        let metadata = flow.metaData
        let identity = context.processIdentity
        let start = identity?.processStartTime.map { "\($0.seconds):\($0.microseconds)" }
        let signing: SignedCodeIdentity? = identity.flatMap {
            if case let .signed(value) = $0.codeSigning { return value }
            return nil
        }
        let source = AppAdmissionSource(processId: identity?.processIdentifier, userId: identity?.effectiveUserID, processStart: start, executablePath: identity?.executablePath, bundleId: signing?.securedBundleIdentifier, signingId: signing?.signingIdentifier ?? (metadata.sourceAppSigningIdentifier.isEmpty ? nil : metadata.sourceAppSigningIdentifier), teamId: signing?.teamIdentifier)
        return AppAdmissionRequest(version: AppAdmissionBootstrap.version, activation: activation, nonce: UUID().uuidString, flowId: flowID.uuidString, kind: kind, network: transport.rawValue, host: resolved.destination.ipAddress?.presentation ?? endpoint.host, hostname: resolved.destination.hostname, port: resolved.destination.port, source: source)
    }

    private func resolveIdentity(_ flow: NEAppProxyFlow) -> ProcessIdentityResolution {
        guard let token = flow.metaData.sourceAppAuditToken else { return .unavailable(.invalidAuditTokenLength(expected: 32, actual: 0)) }
        return identityCache.resolve(sourceAppAuditToken: token, using: identityResolver)
    }
    private func admissionEndpoint(host: String, port: UInt16) -> SOCKS5Endpoint? {
        if let ip = try? IPAddress(host) { return SOCKS5Endpoint(address: SOCKS5Address(ipAddress: ip), port: port) }
        return try? SOCKS5Endpoint(address: SOCKS5Address(domain: host), port: port)
    }
    private func endpointAsSocks(_ endpoint: FlowRemoteEndpoint) -> SOCKS5Endpoint? { admissionEndpoint(host: endpoint.host, port: UInt16(endpoint.port) ?? 0) }
    #endif

    func decideTCPFlow(_ flow: NEAppProxyTCPFlow) -> FlowTrafficDecision {
        planTCPFlow(flow).decision
    }

    func isTrustedMyproxyComponent(_ flow: NEAppProxyFlow) -> Bool {
        if trustedComponentPolicy.contains(
            metadataSigningIdentifier: flow.metaData.sourceAppSigningIdentifier
        ) {
            return true
        }
        guard let auditToken = flow.metaData.sourceAppAuditToken else { return false }
        return trustedComponentPolicy.contains(
            identityCache.resolve(
                sourceAppAuditToken: auditToken,
                using: identityResolver
            )
        )
    }

    /// Reuses application identity matching for a DNS proxy flow. The remote
    /// endpoint is the resolver rather than the queried hostname, so this is
    /// intentionally used only to select an application-scoped Profile route;
    /// unmatched and destination-only rules remain on the default DNS route.
    func decideDNSFlow(
        _ flow: NEAppProxyFlow,
        destination: SOCKS5Endpoint,
        transportProtocol: TransportProtocol
    ) -> FlowTrafficDecision {
        #if MYPROXY_XRAY
        return planDNSFlow(flow, destination: destination, transport: transportProtocol).decision
        #else
        let host = destination.address.ipAddress?.presentation
            ?? destination.address.domain
            ?? ""
        var dnsState = snapshotState()
        dnsState.preparedConfiguration = dnsState.dnsPreparedConfiguration
        dnsState.rulesByIdentifier = dnsState.dnsRulesByIdentifier
        return decide(
            flow: flow,
            endpoint: FlowRemoteEndpoint(
                host: host,
                port: String(destination.port)
            ),
            transportProtocol: transportProtocol,
            state: dnsState,
            remoteHostname: destination.address.domain
        ).decision
        #endif
    }

    #if MYPROXY_XRAY
    func planDNSFlow(_ flow: NEAppProxyFlow, destination: SOCKS5Endpoint, transport: TransportProtocol, parentFlowIdentifier: UUID? = nil) -> (decision: FlowTrafficDecision, target: SOCKS5Endpoint?, proxy: ProviderSOCKSConfiguration?) {
        let endpoint = FlowRemoteEndpoint(host: destination.address.ipAddress?.presentation ?? destination.address.domain ?? "", port: destination.port)
        let outcome = admissionOutcome(flow: flow, endpoint: endpoint, transport: transport, kind: "dns", parentFlowIdentifier: parentFlowIdentifier, useFlowHostname: destination.address.ipAddress != nil)
        return (outcome.decision, outcome.target, outcome.proxy)
    }
    #endif

    @available(macOS 15.0, *)
    func planUDPFlow(
        _ flow: NEAppProxyUDPFlow,
        initialRemoteEndpoint: Network.NWEndpoint,
        parentFlowIdentifier: UUID? = nil
    ) -> UDPFlowInterceptionPlan {
        #if MYPROXY_XRAY
        guard let endpoint = Self.endpoint(initialRemoteEndpoint) else {
            return UDPFlowInterceptionPlan(decision: FlowTrafficDecision(disposition: .reject, reason: .contextUnavailable(.unsupportedRemoteEndpoint)), initialDestination: nil, mihomoDestination: nil, proxy: nil, unavailableFallback: .reject, activity: fallbackActivity(flow: flow, endpoint: nil, transportProtocol: .udp, failure: .unsupportedRemoteEndpoint), parentFlowIdentifier: parentFlowIdentifier)
        }
        return planAdmissionUDP(flow, endpoint: endpoint, parentFlowIdentifier: parentFlowIdentifier ?? UUID())
        #else
        guard let endpoint = Self.endpoint(initialRemoteEndpoint) else {
            return UDPFlowInterceptionPlan(
                decision: failOpen(.unsupportedRemoteEndpoint),
                initialDestination: nil,
                mihomoDestination: nil,
                proxy: nil,
                unavailableFallback: .direct,
                activity: fallbackActivity(
                    flow: flow,
                    endpoint: nil,
                    transportProtocol: .udp,
                    failure: .unsupportedRemoteEndpoint
                ),
                parentFlowIdentifier: parentFlowIdentifier
            )
        }
        return planUDPFlow(
            flow: flow,
            endpoint: endpoint,
            state: snapshotState(),
            parentFlowIdentifier: parentFlowIdentifier
        )
        #endif
    }

    @available(macOS 15.0, *)
    func decideUDPFlow(
        _ flow: NEAppProxyUDPFlow,
        initialRemoteEndpoint: Network.NWEndpoint
    ) -> FlowTrafficDecision {
        planUDPFlow(flow, initialRemoteEndpoint: initialRemoteEndpoint).decision
    }

    @available(macOS, introduced: 14.0, obsoleted: 15.0)
    func planLegacyUDPFlow(
        _ flow: NEAppProxyUDPFlow,
        initialRemoteEndpoint: NetworkExtension.__NWEndpoint,
        parentFlowIdentifier: UUID? = nil
    ) -> UDPFlowInterceptionPlan {
        #if MYPROXY_XRAY
        guard let endpoint = Self.legacyEndpoint(initialRemoteEndpoint) else {
            return UDPFlowInterceptionPlan(decision: FlowTrafficDecision(disposition: .reject, reason: .contextUnavailable(.unsupportedRemoteEndpoint)), initialDestination: nil, mihomoDestination: nil, proxy: nil, unavailableFallback: .reject, activity: fallbackActivity(flow: flow, endpoint: nil, transportProtocol: .udp, failure: .unsupportedRemoteEndpoint), parentFlowIdentifier: parentFlowIdentifier)
        }
        return planAdmissionUDP(flow, endpoint: endpoint, parentFlowIdentifier: parentFlowIdentifier ?? UUID())
        #else
        guard let endpoint = Self.legacyEndpoint(initialRemoteEndpoint) else {
            return UDPFlowInterceptionPlan(
                decision: failOpen(.unsupportedRemoteEndpoint),
                initialDestination: nil,
                mihomoDestination: nil,
                proxy: nil,
                unavailableFallback: .direct,
                activity: fallbackActivity(
                    flow: flow,
                    endpoint: nil,
                    transportProtocol: .udp,
                    failure: .unsupportedRemoteEndpoint
                ),
                parentFlowIdentifier: parentFlowIdentifier
            )
        }
        return planUDPFlow(
            flow: flow,
            endpoint: endpoint,
            state: snapshotState(),
            parentFlowIdentifier: parentFlowIdentifier
        )
        #endif
    }

    /// Re-evaluates one destination of an already-owned UDP flow. A UDP socket
    /// may send datagrams to several endpoints, so the initial flow decision is
    /// not a safe substitute for a per-destination rule decision.
    func planUDPDatagram(
        _ flow: NEAppProxyUDPFlow,
        destination: SOCKS5Endpoint,
        parentFlowIdentifier: UUID
    ) -> UDPFlowInterceptionPlan {
        #if MYPROXY_XRAY
        return planAdmissionUDP(flow, endpoint: FlowRemoteEndpoint(host: destination.address.ipAddress?.presentation ?? destination.address.domain ?? "", port: destination.port), parentFlowIdentifier: parentFlowIdentifier, useFlowHostname: false)
        #else
        let endpoint = FlowRemoteEndpoint(
            host: destination.address.ipAddress?.presentation
                ?? destination.address.domain
                ?? "",
            port: destination.port
        )
        return planUDPFlow(
            flow: flow,
            endpoint: endpoint,
            state: snapshotState(),
            parentFlowIdentifier: parentFlowIdentifier,
            // An NE UDP flow's remoteHostname describes its initial target and
            // must not leak into later per-datagram destination decisions.
            remoteHostname: ""
        )
        #endif
    }

    func currentRevision() -> UInt64 {
        snapshotState().revision
    }

    @available(macOS, introduced: 14.0, obsoleted: 15.0)
    func decideLegacyUDPFlow(
        _ flow: NEAppProxyUDPFlow,
        initialRemoteEndpoint: NetworkExtension.__NWEndpoint
    ) -> FlowTrafficDecision {
        planLegacyUDPFlow(flow, initialRemoteEndpoint: initialRemoteEndpoint).decision
    }

    func failOpen(_ failure: FlowContextConversionFailure) -> FlowTrafficDecision {
        #if MYPROXY_XRAY
        return FlowTrafficDecision(disposition: .reject, reason: .contextUnavailable(failure))
        #else
        let currentState = snapshotState()
        return decisionAdapter.decide(
            preparedConfiguration: currentState.preparedConfiguration,
            context: .failOpen(failure),
            captureEnabled: currentState.captureEnabled,
            mihomoAvailable: false
        )
        #endif
    }

    private func decide(
        flow: NEAppProxyFlow,
        endpoint: FlowRemoteEndpoint,
        transportProtocol: TransportProtocol,
        state currentState: State,
        remoteHostname: String? = nil,
        activityFlowIdentifier: UUID? = nil,
        parentFlowIdentifier: UUID? = nil
    ) -> FlowDecisionOutcome {
        let metadata = flow.metaData
        let applicationMetadata = FlowApplicationMetadata(
            sourceAppAuditToken: metadata.sourceAppAuditToken,
            sourceAppUniqueIdentifier: metadata.sourceAppUniqueIdentifier,
            sourceAppSigningIdentifier: metadata.sourceAppSigningIdentifier
        )
        let identityResolution: ProcessIdentityResolution
        if let auditTokenData = applicationMetadata.sourceAppAuditToken {
            identityResolution = identityCache.resolve(
                sourceAppAuditToken: auditTokenData,
                using: identityResolver
            )
        } else {
            identityResolution = .unavailable(.invalidAuditTokenLength(expected: 32, actual: 0))
        }
        let isTrustedMyproxyComponent = trustedComponentPolicy.contains(identityResolution)
            || trustedComponentPolicy.contains(
                metadataSigningIdentifier: applicationMetadata.sourceAppSigningIdentifier
            )
        let context = contextBuilder.resolve(
            endpoint: endpoint,
            remoteHostname: remoteHostname ?? flow.remoteHostname,
            metadata: applicationMetadata,
            identityResolution: identityResolution,
            transportProtocol: transportProtocol,
            isTrustedMyproxyComponent: isTrustedMyproxyComponent
        )
        let preliminaryDecision = decisionAdapter.decide(
            preparedConfiguration: currentState.preparedConfiguration,
            context: context,
            captureEnabled: currentState.captureEnabled,
            mihomoAvailable: !currentState.mihomoSOCKSConfigurations.isEmpty
        )
        let decision = MihomoRouteAvailabilityPolicy.resolve(
            preliminaryDecision,
            availableRoutes: currentState.availableMihomoRoutes,
            rulesByIdentifier: currentState.rulesByIdentifier
        )
        return FlowDecisionOutcome(
            decision: decision,
            destinationHostname: context.context?.destination.hostname,
            activity: makeActivity(
                flow: flow,
                endpoint: endpoint,
                transportProtocol: transportProtocol,
                context: context,
                identityResolution: identityResolution,
                decision: decision,
                state: currentState,
                flowIdentifier: activityFlowIdentifier,
                parentFlowIdentifier: parentFlowIdentifier
            )
        )
    }

    private func planUDPFlow(
        flow: NEAppProxyUDPFlow,
        endpoint: FlowRemoteEndpoint,
        state currentState: State,
        parentFlowIdentifier: UUID? = nil,
        remoteHostname: String? = nil
    ) -> UDPFlowInterceptionPlan {
        let outcome = decide(
            flow: flow,
            endpoint: endpoint,
            transportProtocol: .udp,
            state: currentState,
            remoteHostname: remoteHostname,
            parentFlowIdentifier: parentFlowIdentifier
        )
        let routePlan: ProviderSOCKSFlowPlan?
        if case .reject = outcome.decision.disposition,
           let destinations = try? ProviderSOCKSConfiguration.destinations(
               for: endpoint,
               preferredHostname: outcome.destinationHostname
           ) {
            routePlan = ProviderSOCKSFlowPlan(
                destinations: destinations,
                proxy: nil
            )
        } else {
            routePlan = try? ProviderSOCKSConfiguration.flowPlan(
                for: outcome.decision,
                endpoint: endpoint,
                preferredHostname: outcome.destinationHostname,
                routeCatalog: currentState.mihomoSOCKSConfigurations
            )
        }
        return UDPFlowInterceptionPlan(
            decision: outcome.decision,
            initialDestination: routePlan?.destinations.original,
            mihomoDestination: routePlan.map {
                DNSProxyUpstreamResolver.relayDestination(
                    for: $0.destinations.mihomo,
                    resolvers: [$0.destinations.original]
                )
            },
            proxy: routePlan?.proxy,
            unavailableFallback: unavailableFallbackRequested(
                by: outcome.decision,
                rulesByIdentifier: currentState.rulesByIdentifier
            ),
            activity: outcome.activity
        )
    }

    private func makeActivity(
        flow: NEAppProxyFlow,
        endpoint: FlowRemoteEndpoint,
        transportProtocol: TransportProtocol,
        context: FlowContextResolution,
        identityResolution: ProcessIdentityResolution,
        decision: FlowTrafficDecision,
        state: State,
        flowIdentifier: UUID? = nil,
        parentFlowIdentifier: UUID? = nil
    ) -> AppRoutingActivity {
        let resolvedContext = context.context
        let identity = identityResolution.identity
        let signing: SignedCodeIdentity?
        if case let .signed(value) = identity?.codeSigning {
            signing = value
        } else {
            signing = nil
        }
        let source = resolvedContext?.source
        let resolvedDestination = resolvedContext?.destination
        let endpointHost = endpoint.host.trimmingCharacters(in: .whitespacesAndNewlines)
        let endpointAddress = try? IPAddress(endpointHost.trimmingCharacters(in: CharacterSet(charactersIn: "[]")))
        let configuredAction = actionRequested(
            by: decision,
            rulesByIdentifier: state.rulesByIdentifier
        )
        let terminal: Bool = switch decision.disposition {
        case .mihomo: false
        case .direct, .reject, .failOpen: true
        }

        return AppRoutingActivity(
            flowIdentifier: flowIdentifier ?? UUID(),
            parentFlowIdentifier: parentFlowIdentifier,
            configurationRevision: state.revision,
            startedAt: Date(),
            endedAt: terminal ? Date() : nil,
            source: AppRoutingActivitySource(
                processIdentifier: source?.processIdentifier ?? identity?.processIdentifier ?? 0,
                processStartTime: source?.processStartTime ?? identity?.processStartTime,
                userIdentifier: source?.userID ?? identity?.effectiveUserID ?? 0,
                executablePath: source?.executablePath ?? identity?.executablePath,
                bundleIdentifier: source?.bundleIdentifier ?? signing?.securedBundleIdentifier,
                signingIdentifier: source?.signingIdentifier ?? signing?.signingIdentifier,
                teamIdentifier: source?.teamIdentifier ?? signing?.teamIdentifier
            ),
            destination: AppRoutingActivityDestination(
                hostname: resolvedDestination?.hostname ?? flow.remoteHostname,
                ipAddress: resolvedDestination?.ipAddress?.presentation ?? endpointAddress?.presentation,
                port: resolvedDestination?.port ?? UInt16(endpoint.port) ?? 0
            ),
            transportProtocol: transportProtocol,
            decision: decision,
            configuredAction: configuredAction,
            effectiveAction: decision.disposition,
            relayState: terminal ? .notApplicable : .pending
        )
    }

    private func fallbackActivity(
        flow: NEAppProxyFlow,
        endpoint: FlowRemoteEndpoint?,
        transportProtocol: TransportProtocol,
        failure: FlowContextConversionFailure
    ) -> AppRoutingActivity {
        let currentState = snapshotState()
        let decision = failOpen(failure)
        let metadata = flow.metaData
        return AppRoutingActivity(
            configurationRevision: currentState.revision,
            startedAt: Date(),
            endedAt: Date(),
            source: AppRoutingActivitySource(
                processIdentifier: 0,
                userIdentifier: 0,
                signingIdentifier: metadata.sourceAppSigningIdentifier
            ),
            destination: AppRoutingActivityDestination(
                hostname: flow.remoteHostname,
                ipAddress: endpoint?.host,
                port: endpoint.flatMap { UInt16($0.port) } ?? 0
            ),
            transportProtocol: transportProtocol,
            decision: decision,
            configuredAction: .direct,
            effectiveAction: .failOpen,
            relayState: .notApplicable,
            relayError: failure.description
        )
    }

    private func actionRequested(
        by decision: FlowTrafficDecision,
        rulesByIdentifier: [String: CaptureRule]
    ) -> CaptureAction {
        let cause: RuleDecisionCause?
        switch decision.reason {
        case let .rule(value):
            cause = value
        case let .mihomoUnavailable(rule, _):
            cause = rule
        case .captureDisabled, .configurationUnavailable, .contextUnavailable:
            cause = nil
        }
        if case let .matchedRule(identifier) = cause,
           let rule = rulesByIdentifier[identifier] {
            return rule.action
        }
        return switch decision.disposition {
        case .reject: .reject
        case let .mihomo(route): .mihomo(route)
        case .direct, .failOpen: .direct
        }
    }

    private func unavailableFallbackRequested(
        by decision: FlowTrafficDecision,
        rulesByIdentifier: [String: CaptureRule]
    ) -> UnavailableFallback {
        let cause: RuleDecisionCause?
        switch decision.reason {
        case let .rule(value):
            cause = value
        case let .mihomoUnavailable(rule, fallback):
            if case .matchedRule = rule {
                return fallback
            }
            cause = rule
        case .captureDisabled, .configurationUnavailable, .contextUnavailable:
            cause = nil
        }
        if case let .matchedRule(identifier) = cause,
           let rule = rulesByIdentifier[identifier] {
            return rule.unavailableFallback
        }
        return .direct
    }

    private func snapshotState() -> State {
        lock.lock()
        defer { lock.unlock() }
        return state
    }

    @available(macOS 15.0, *)
    private static func endpoint(_ endpoint: Network.NWEndpoint) -> FlowRemoteEndpoint? {
        guard case let .hostPort(host, port) = endpoint else { return nil }
        return FlowRemoteEndpoint(host: host.debugDescription, port: port.rawValue)
    }

    #if MYPROXY_XRAY
    private static func endpointTCP(_ flow: NEAppProxyTCPFlow) -> FlowRemoteEndpoint? {
        if #available(macOS 15.0, *) { return endpoint(flow.remoteFlowEndpoint) }
        return legacyEndpoint(flow.__remoteEndpoint)
    }
    #endif

    @available(macOS, introduced: 14.0, obsoleted: 15.0)
    private static func legacyEndpoint(
        _ endpoint: NetworkExtension.__NWEndpoint
    ) -> FlowRemoteEndpoint? {
        // Swift 6 hides the deprecated NWHostEndpoint wrapper. KVC keeps the
        // macOS 14 compatibility path isolated without importing deprecated
        // members into the strict-concurrency build.
        let object = endpoint as NSObject
        guard object.isKind(of: NetworkExtension.__NWHostEndpoint.self),
              let host = object.value(forKey: "hostname") as? String,
              let port = object.value(forKey: "port") as? String
        else {
            return nil
        }
        return FlowRemoteEndpoint(host: host, port: port)
    }

    private static func bool(_ value: Any?) -> Bool? {
        switch value {
        case let value as Bool: value
        case let value as NSNumber: value.boolValue
        case let value as String:
            switch value.lowercased() {
            case "true", "yes", "1": true
            case "false", "no", "0": false
            default: nil
            }
        default: nil
        }
    }

    private static func uint64(_ value: Any?) -> UInt64? {
        switch value {
        case let value as UInt64: value
        case let value as Int where value >= 0: UInt64(value)
        case let value as NSNumber where value.int64Value >= 0: value.uint64Value
        case let value as String: UInt64(value)
        default: nil
        }
    }
}

private struct FlowDecisionOutcome: Sendable {
    let decision: FlowTrafficDecision
    let destinationHostname: String?
    let activity: AppRoutingActivity
}

struct TCPFlowInterceptionPlan: Sendable {
    let decision: FlowTrafficDecision
    /// Original macOS endpoint, retained for Direct and unavailable fallback.
    let destination: SOCKS5Endpoint?
    /// Hostname-preserving SOCKS target used only for Mihomo relay.
    let mihomoDestination: SOCKS5Endpoint?
    let proxy: ProviderSOCKSConfiguration?
    let unavailableFallback: UnavailableFallback
    let activity: AppRoutingActivity
}

struct UDPFlowInterceptionPlan: Sendable {
    let decision: FlowTrafficDecision
    /// Original datagram endpoint, retained as the conversation key and for Direct.
    let initialDestination: SOCKS5Endpoint?
    /// Hostname-preserving SOCKS target used only for Mihomo relay.
    let mihomoDestination: SOCKS5Endpoint?
    /// Target a Direct or unavailable-fallback relay dials when it differs from
    /// the original endpoint. The DNS provider sets this when macOS reports the
    /// queried name instead of the resolver the flow actually addresses.
    let directDestination: SOCKS5Endpoint?
    let proxy: ProviderSOCKSConfiguration?
    let unavailableFallback: UnavailableFallback
    let activity: AppRoutingActivity
    let parentFlowIdentifier: UUID?

    init(
        decision: FlowTrafficDecision,
        initialDestination: SOCKS5Endpoint?,
        mihomoDestination: SOCKS5Endpoint?,
        directDestination: SOCKS5Endpoint? = nil,
        proxy: ProviderSOCKSConfiguration?,
        unavailableFallback: UnavailableFallback,
        activity: AppRoutingActivity,
        parentFlowIdentifier: UUID? = nil
    ) {
        self.decision = decision
        self.initialDestination = initialDestination
        self.mihomoDestination = mihomoDestination
        self.directDestination = directDestination
        self.proxy = proxy
        self.unavailableFallback = unavailableFallback
        self.activity = activity
        self.parentFlowIdentifier = parentFlowIdentifier ?? activity.parentFlowIdentifier
    }
}
