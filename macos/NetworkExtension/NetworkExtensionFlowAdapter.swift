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
        let fallback: UnavailableFallback
        if case let .matchedRule(identifier) = cause,
           let rule = rulesByIdentifier[identifier] {
            fallback = rule.unavailableFallback
        } else {
            fallback = .direct
        }
        let disposition: FlowTrafficDisposition = switch fallback {
        case .direct: .direct
        case .reject: .reject
        }
        return FlowTrafficDecision(
            disposition: disposition,
            reason: .mihomoUnavailable(rule: cause, fallback: fallback),
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
    }

    private let lock = NSLock()
    private let identityResolver = ProcessIdentityResolver()
    private let identityCache = ProcessIdentityResolutionCache(capacity: 256)
    private let trustedComponentPolicy = TrustedMyproxyComponentPolicy()
    private let contextBuilder = FlowContextBuilder()
    private let decisionAdapter = FlowTrafficDecisionAdapter()
    private var state = State()

    func load(configuration: [String: Any]?) {
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
               rules: dnsRules
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
            mihomoDestination: routePlan?.destinations.mihomo,
            proxy: routePlan?.proxy,
            unavailableFallback: unavailableFallbackRequested(
                by: outcome.decision,
                rulesByIdentifier: currentState.rulesByIdentifier
            ),
            activity: outcome.activity
        )
    }

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
    }

    @available(macOS 15.0, *)
    func planUDPFlow(
        _ flow: NEAppProxyUDPFlow,
        initialRemoteEndpoint: Network.NWEndpoint,
        parentFlowIdentifier: UUID? = nil
    ) -> UDPFlowInterceptionPlan {
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
    }

    /// Re-evaluates one destination of an already-owned UDP flow. A UDP socket
    /// may send datagrams to several endpoints, so the initial flow decision is
    /// not a safe substitute for a per-destination rule decision.
    func planUDPDatagram(
        _ flow: NEAppProxyUDPFlow,
        destination: SOCKS5Endpoint,
        parentFlowIdentifier: UUID
    ) -> UDPFlowInterceptionPlan {
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
        let currentState = snapshotState()
        return decisionAdapter.decide(
            preparedConfiguration: currentState.preparedConfiguration,
            context: .failOpen(failure),
            captureEnabled: currentState.captureEnabled,
            mihomoAvailable: false
        )
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
            mihomoDestination: routePlan?.destinations.mihomo,
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
    let proxy: ProviderSOCKSConfiguration?
    let unavailableFallback: UnavailableFallback
    let activity: AppRoutingActivity
    let parentFlowIdentifier: UUID?

    init(
        decision: FlowTrafficDecision,
        initialDestination: SOCKS5Endpoint?,
        mihomoDestination: SOCKS5Endpoint?,
        proxy: ProviderSOCKSConfiguration?,
        unavailableFallback: UnavailableFallback,
        activity: AppRoutingActivity,
        parentFlowIdentifier: UUID? = nil
    ) {
        self.decision = decision
        self.initialDestination = initialDestination
        self.mihomoDestination = mihomoDestination
        self.proxy = proxy
        self.unavailableFallback = unavailableFallback
        self.activity = activity
        self.parentFlowIdentifier = parentFlowIdentifier ?? activity.parentFlowIdentifier
    }
}
