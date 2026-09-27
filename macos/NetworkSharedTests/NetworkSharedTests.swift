import Foundation

// The repo has no SwiftPM or Xcode test target, so this is a plain executable harness.

enum ExpectationFailure: Error, CustomStringConvertible {
    case message(String)

    var description: String {
        switch self {
        case let .message(text): text
        }
    }
}

func expect(_ condition: @autoclosure () -> Bool, _ message: String) throws {
    if !condition() {
        throw ExpectationFailure.message(message)
    }
}

func expectEqual<T: Equatable>(_ actual: T, _ expected: T, _ label: String) throws {
    if actual != expected {
        throw ExpectationFailure.message("\(label): got \(actual), expected \(expected)")
    }
}

func expectBytes(_ actual: Data, _ expected: [UInt8], _ label: String) throws {
    let actualBytes = Array(actual)
    if actualBytes != expected {
        throw ExpectationFailure.message(
            "\(label): got [\(actualBytes.map { String(format: "0x%02X", $0) }.joined(separator: ", "))], "
                + "expected [\(expected.map { String(format: "0x%02X", $0) }.joined(separator: ", "))]"
        )
    }
}

func expectThrows<E: Error & Equatable>(
    _ expected: E,
    _ label: String,
    _ body: () throws -> Void
) throws {
    do {
        try body()
        throw ExpectationFailure.message("\(label): expected throw \(expected), got success")
    } catch let error as E {
        try expectEqual(error, expected, label)
    } catch let failure as ExpectationFailure {
        throw failure
    } catch {
        throw ExpectationFailure.message("\(label): expected \(expected), got \(error)")
    }
}

struct TestRun {
    var passed = 0
    var failed = 0

    mutating func test(_ name: String, _ body: () throws -> Void) {
        do {
            try body()
            passed += 1
            print("ok \(name)")
        } catch {
            failed += 1
            print("FAIL \(name): \(error)")
        }
    }
}

private func emptyAuditToken() -> Data {
    Data(repeating: 0, count: 32)
}

private func flowSource(
    executablePath: String? = nil,
    designatedRequirement: String? = nil,
    signingIdentifier: String? = nil,
    bundleIdentifier: String? = nil,
    isTrustedMyproxyComponent: Bool = false
) -> FlowSource {
    FlowSource(
        processIdentifier: 4242,
        auditToken: emptyAuditToken(),
        userID: 501,
        executablePath: executablePath,
        designatedRequirement: designatedRequirement,
        signingIdentifier: signingIdentifier,
        bundleIdentifier: bundleIdentifier,
        isTrustedMyproxyComponent: isTrustedMyproxyComponent
    )
}

private func engine(rules: [CaptureRule], capturePrivateNetworks: Bool = false) throws -> CaptureRuleEngine {
    try CaptureRuleEngine(
        snapshot: CaptureConfigurationSnapshot(
            revision: 1,
            rules: rules,
            capturePrivateNetworks: capturePrivateNetworks
        )
    )
}

private func context(
    source: FlowSource,
    destination: FlowDestination,
    transportProtocol: TransportProtocol = .tcp
) -> FlowContext {
    FlowContext(source: source, destination: destination, transportProtocol: transportProtocol)
}

@main
struct NetworkSharedTests {
    static func main() {
        var run = TestRun()

        run.test("socks5_encode_greeting_no_auth") {
            let data = try SOCKS5Codec.encodeGreeting(methods: [.noAuthenticationRequired])
            try expectBytes(data, [0x05, 0x01, 0x00], "greeting")
        }

        run.test("socks5_encode_connect_ipv4") {
            let endpoint = SOCKS5Endpoint(
                address: SOCKS5Address(ipAddress: try IPAddress("1.2.3.4")),
                port: 443
            )
            let request = try SOCKS5CommandRequest(command: .connect, endpoint: endpoint)
            let data = try SOCKS5Codec.encodeCommandRequest(request)
            try expectBytes(
                data,
                [0x05, 0x01, 0x00, 0x01, 0x01, 0x02, 0x03, 0x04, 0x01, 0xBB],
                "connect ipv4"
            )
        }

        run.test("socks5_encode_connect_ipv6") {
            let endpoint = SOCKS5Endpoint(
                address: SOCKS5Address(ipAddress: try IPAddress("2001:db8::1")),
                port: 80
            )
            let request = try SOCKS5CommandRequest(command: .connect, endpoint: endpoint)
            let data = try SOCKS5Codec.encodeCommandRequest(request)
            try expectBytes(
                data,
                [
                    0x05, 0x01, 0x00, 0x04,
                    0x20, 0x01, 0x0D, 0xB8, 0x00, 0x00, 0x00, 0x00,
                    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
                    0x00, 0x50,
                ],
                "connect ipv6"
            )
        }

        run.test("socks5_encode_connect_domain") {
            let endpoint = SOCKS5Endpoint(
                address: try SOCKS5Address(domain: "example.com"),
                port: 443
            )
            let request = try SOCKS5CommandRequest(command: .connect, endpoint: endpoint)
            let data = try SOCKS5Codec.encodeCommandRequest(request)
            var expected: [UInt8] = [0x05, 0x01, 0x00, 0x03, 0x0B]
            expected.append(contentsOf: Array("example.com".utf8))
            expected.append(contentsOf: [0x01, 0xBB])
            try expectBytes(data, expected, "connect domain")
        }

        run.test("socks5_reply_decoder_keeps_coalesced_payload") {
            var decoder = SOCKS5CommandReplyDecoder()
            var frame: [UInt8] = [
                0x05, 0x00, 0x00, 0x01,
                0x00, 0x00, 0x00, 0x00,
                0x00, 0x00,
            ]
            frame.append(contentsOf: Array("HELLO".utf8))
            let reply = try decoder.append(Data(frame))
            try expect(reply != nil, "reply should complete")
            try expectEqual(reply!.code, .succeeded, "reply code")
            try expectEqual(reply!.boundEndpoint.port, 0, "bound port")
            try expectEqual(
                reply!.boundEndpoint.address.ipAddress?.presentation,
                "0.0.0.0",
                "bound address"
            )
            try expectBytes(decoder.remainingData, Array("HELLO".utf8), "remaining payload")
            try expect(decoder.isComplete, "decoder complete")
        }

        run.test("socks5_reply_rejects_bad_version") {
            try expectThrows(SOCKS5CodecError.invalidVersion(0x04), "bad version") {
                _ = try SOCKS5Codec.decodeCommandReply(
                    Data([0x04, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00])
                )
            }
        }

        run.test("socks5_reply_rejects_oversize_input") {
            let limit = SOCKS5Limits.maximumStreamInputBytes
            let oversized = Data(repeating: 0x05, count: limit + 1)
            try expectThrows(
                SOCKS5CodecError.inputTooLarge(limit: limit, actual: limit + 1),
                "oversize"
            ) {
                _ = try SOCKS5Codec.decodeCommandReply(oversized)
            }
        }

        run.test("socks5_udp_datagram_round_trip") {
            let destination = SOCKS5Endpoint(
                address: SOCKS5Address(ipAddress: try IPAddress("8.8.4.4")),
                port: 53
            )
            let original = try SOCKS5UDPDatagram(
                destination: destination,
                payload: Data([0xDE, 0xAD, 0xBE, 0xEF])
            )
            let encoded = try SOCKS5Codec.encodeUDPDatagram(original)
            try expectBytes(
                encoded,
                [
                    0x00, 0x00, 0x00, 0x01,
                    0x08, 0x08, 0x04, 0x04,
                    0x00, 0x35,
                    0xDE, 0xAD, 0xBE, 0xEF,
                ],
                "udp wire"
            )
            let decoded = try SOCKS5Codec.decodeUDPDatagram(encoded)
            try expectEqual(decoded, original, "udp round trip")
        }

        run.test("socks5_udp_rejects_fragment") {
            let bytes: [UInt8] = [
                0x00, 0x00, 0x01, 0x01,
                0x08, 0x08, 0x08, 0x08,
                0x00, 0x35,
                0x01,
            ]
            try expectThrows(SOCKS5CodecError.fragmentedUDPDatagram(0x01), "udp fragment") {
                _ = try SOCKS5Codec.decodeUDPDatagram(Data(bytes))
            }
        }

        run.test("capture_exact_application_match") {
            let requirement = "anchor apple generic and identifier \"com.example.app\""
            let rule = try CaptureRule(
                id: "exact-app",
                priority: 10,
                sources: [
                    .application(
                        ApplicationSourceMatcher(
                            designatedRequirement: requirement,
                            bundleIdentifier: "com.example.app"
                        )
                    ),
                ],
                action: .reject
            )
            let engine = try engine(rules: [rule])
            let matched = engine.evaluate(
                context(
                    source: flowSource(
                        designatedRequirement: requirement,
                        bundleIdentifier: "com.example.app"
                    ),
                    destination: try FlowDestination(hostname: "example.com", port: 443)
                )
            )
            try expectEqual(matched.cause, .matchedRule("exact-app"), "exact app cause")
            try expectEqual(matched.action, .reject, "exact app action")

            let missed = engine.evaluate(
                context(
                    source: flowSource(
                        designatedRequirement: "different requirement",
                        bundleIdentifier: "com.example.app"
                    ),
                    destination: try FlowDestination(hostname: "example.com", port: 443)
                )
            )
            try expectEqual(missed.cause, .defaultDirect, "exact app miss")
        }

        run.test("capture_name_wildcard_matches_electron_helper") {
            let rule = try CaptureRule(
                id: "t3-helper",
                priority: 10,
                sources: [
                    .applicationIdentifierPattern(
                        try ApplicationIdentifierPatternMatcher(pattern: "T3 Code*")
                    ),
                ],
                action: .mihomo(.group("PROXY"))
            )
            let engine = try engine(rules: [rule])
            let decision = engine.evaluate(
                context(
                    source: flowSource(
                        executablePath:
                            "/Applications/T3 Code (Nightly).app/Contents/Frameworks/"
                            + "T3 Code (Nightly) Helper"
                    ),
                    destination: try FlowDestination(hostname: "api.example.com", port: 443)
                )
            )
            try expectEqual(decision.cause, .matchedRule("t3-helper"), "wildcard cause")
            try expectEqual(decision.action, .mihomo(.group("PROXY")), "wildcard action")
            try expectEqual(
                decision.evidence?.source,
                .applicationIdentifierPattern(
                    RuleApplicationPatternEvidence(
                        pattern: "t3 code*",
                        matchedField: .executableName
                    )
                ),
                "wildcard evidence"
            )
        }

        run.test("capture_bundle_id_match") {
            let rule = try CaptureRule(
                id: "bundle",
                priority: 10,
                sources: [
                    .applicationIdentifierPattern(
                        try ApplicationIdentifierPatternMatcher(pattern: "com.t3tools.t3code")
                    ),
                ],
                action: .reject
            )
            let engine = try engine(rules: [rule])
            let hit = engine.evaluate(
                context(
                    source: flowSource(bundleIdentifier: "com.t3tools.t3code"),
                    destination: try FlowDestination(hostname: "example.com", port: 443)
                )
            )
            try expectEqual(hit.cause, .matchedRule("bundle"), "bundle hit")

            let miss = engine.evaluate(
                context(
                    source: flowSource(bundleIdentifier: "com.other.app"),
                    destination: try FlowDestination(hostname: "example.com", port: 443)
                )
            )
            try expectEqual(miss.cause, .defaultDirect, "bundle miss")
        }

        run.test("capture_domain_exact_vs_suffix") {
            let exact = try CaptureRule(
                id: "exact-host",
                priority: 10,
                destinations: [.host(try HostMatcher(kind: .exact, value: "telegram.org"))],
                action: .reject
            )
            let suffix = try CaptureRule(
                id: "suffix-host",
                priority: 20,
                destinations: [.host(try HostMatcher(kind: .suffix, value: "telegram.org"))],
                action: .mihomo(.group("Telegram"))
            )
            let engine = try engine(rules: [exact, suffix])

            let exactHit = engine.evaluate(
                context(
                    source: flowSource(),
                    destination: try FlowDestination(hostname: "telegram.org", port: 443)
                )
            )
            try expectEqual(exactHit.cause, .matchedRule("exact-host"), "exact host")

            let suffixHit = engine.evaluate(
                context(
                    source: flowSource(),
                    destination: try FlowDestination(hostname: "api.telegram.org", port: 443)
                )
            )
            try expectEqual(suffixHit.cause, .matchedRule("suffix-host"), "suffix host")
            try expectEqual(suffixHit.action, .mihomo(.group("Telegram")), "suffix action")

            let nonMatch = engine.evaluate(
                context(
                    source: flowSource(),
                    destination: try FlowDestination(hostname: "nottelegram.org", port: 443)
                )
            )
            try expectEqual(nonMatch.cause, .defaultDirect, "nottelegram.org must miss")
        }

        run.test("capture_cidr_ipv4_and_ipv6") {
            let v4 = try CaptureRule(
                id: "cidr-v4",
                priority: 10,
                destinations: [.network(try IPNetwork("8.8.8.0/24"))],
                action: .reject
            )
            let v6 = try CaptureRule(
                id: "cidr-v6",
                priority: 20,
                destinations: [.network(try IPNetwork("2001:db8::/32"))],
                action: .mihomo(.group("PROXY"))
            )
            let engine = try engine(rules: [v4, v6])

            let ipv4Hit = engine.evaluate(
                context(
                    source: flowSource(),
                    destination: try FlowDestination(ipAddress: try IPAddress("8.8.8.8"), port: 53)
                )
            )
            try expectEqual(ipv4Hit.cause, .matchedRule("cidr-v4"), "ipv4 cidr")

            let ipv4Miss = engine.evaluate(
                context(
                    source: flowSource(),
                    destination: try FlowDestination(ipAddress: try IPAddress("8.8.4.4"), port: 53)
                )
            )
            try expectEqual(ipv4Miss.cause, .defaultDirect, "ipv4 cidr miss")

            let ipv6Hit = engine.evaluate(
                context(
                    source: flowSource(),
                    destination: try FlowDestination(
                        ipAddress: try IPAddress("2001:db8::abcd"),
                        port: 443
                    )
                )
            )
            try expectEqual(ipv6Hit.cause, .matchedRule("cidr-v6"), "ipv6 cidr")
        }

        run.test("capture_first_match_wins_by_priority") {
            let first = try CaptureRule(
                id: "first",
                priority: 1,
                destinations: [.host(try HostMatcher(kind: .suffix, value: "example.com"))],
                action: .reject
            )
            let second = try CaptureRule(
                id: "second",
                priority: 50,
                destinations: [.host(try HostMatcher(kind: .suffix, value: "example.com"))],
                action: .mihomo(.group("PROXY"))
            )
            let engine = try engine(rules: [second, first])
            let decision = engine.evaluate(
                context(
                    source: flowSource(),
                    destination: try FlowDestination(hostname: "www.example.com", port: 443)
                )
            )
            try expectEqual(decision.cause, .matchedRule("first"), "priority order")
            try expectEqual(decision.action, .reject, "priority action")
        }

        run.test("capture_no_match_defaults_to_direct") {
            let rule = try CaptureRule(
                id: "only-google",
                priority: 10,
                destinations: [.host(try HostMatcher(kind: .exact, value: "google.com"))],
                action: .reject
            )
            let engine = try engine(rules: [rule])
            let decision = engine.evaluate(
                context(
                    source: flowSource(),
                    destination: try FlowDestination(hostname: "example.org", port: 443)
                )
            )
            try expectEqual(decision.cause, .defaultDirect, "no match cause")
            try expectEqual(decision.action, .direct, "no match action")
            try expectEqual(
                decision.evidence?.outcome,
                .defaultDirect,
                "no match evidence"
            )
        }

        run.test("capture_trusted_component_bypasses_matching_rule") {
            let rule = try CaptureRule(
                id: "all-example",
                priority: 10,
                destinations: [.host(try HostMatcher(kind: .suffix, value: "example.com"))],
                action: .mihomo(.group("PROXY"))
            )
            let decision = try engine(rules: [rule]).evaluate(
                context(
                    source: flowSource(isTrustedMyproxyComponent: true),
                    destination: try FlowDestination(hostname: "www.example.com", port: 443)
                )
            )
            try expectEqual(decision.cause, .builtInBypass(.trustedmyproxyComponent), "trusted cause")
            try expectEqual(decision.action, .direct, "trusted action")
        }

        run.test("capture_local_destinations_bypass_rules") {
            let rules = [
                try CaptureRule(
                    id: "loopback",
                    priority: 10,
                    destinations: [.network(try IPNetwork("127.0.0.0/8"))],
                    action: .reject
                ),
                try CaptureRule(
                    id: "lan",
                    priority: 20,
                    destinations: [.network(try IPNetwork("192.168.0.0/16"))],
                    action: .reject
                ),
                try CaptureRule(
                    id: "local-suffix",
                    priority: 30,
                    destinations: [.host(try HostMatcher(kind: .suffix, value: "local"))],
                    action: .reject
                ),
            ]
            let lan = try FlowDestination(ipAddress: try IPAddress("192.168.1.10"), port: 80)
            let defaults = try engine(rules: rules)
            try expectEqual(
                defaults.evaluate(context(
                    source: flowSource(),
                    destination: try FlowDestination(ipAddress: try IPAddress("127.0.0.1"), port: 7890)
                )).cause,
                .builtInBypass(.loopback),
                "loopback"
            )
            try expectEqual(
                defaults.evaluate(context(source: flowSource(), destination: lan)).cause,
                .builtInBypass(.privateNetwork),
                "private network by default"
            )
            try expectEqual(
                defaults.evaluate(context(
                    source: flowSource(),
                    destination: try FlowDestination(hostname: "printer.local", port: 631)
                )).cause,
                .builtInBypass(.localHostname),
                "local hostname"
            )
            let lanCapture = try engine(rules: rules, capturePrivateNetworks: true)
            try expectEqual(
                lanCapture.evaluate(context(source: flowSource(), destination: lan)).cause,
                .matchedRule("lan"),
                "private network when LAN capture is on"
            )
        }

        run.test("capture_rule_carries_profile_rules_fallback") {
            let rule = try CaptureRule(
                id: "group-pin",
                priority: 10,
                destinations: [.host(try HostMatcher(kind: .suffix, value: "example.com"))],
                action: .mihomo(.group("Telegram")),
                unavailableFallback: .profileRules
            )
            let decision = try engine(rules: [rule]).evaluate(
                context(
                    source: flowSource(),
                    destination: try FlowDestination(hostname: "api.example.com", port: 443)
                )
            )
            try expectEqual(decision.unavailableFallback, .profileRules, "fallback carried")
        }

        func dnsQuery(_ labels: [String]) -> Data {
            var bytes: [UInt8] = [0x12, 0x34, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
            for label in labels {
                bytes.append(UInt8(label.utf8.count))
                bytes.append(contentsOf: label.utf8)
            }
            bytes.append(contentsOf: [0x00, 0x00, 0x01, 0x00, 0x01])
            return Data(bytes)
        }

        run.test("dns_query_scope_from_message") {
            func scope(_ message: Data) -> DNSQueryScope? {
                DNSQueryScope.questionName(in: message).map { DNSQueryScope(name: $0) }
            }
            try expectEqual(
                DNSQueryScope.questionName(in: dnsQuery(["www", "youtube", "com"])),
                "www.youtube.com",
                "question name"
            )
            try expectEqual(scope(dnsQuery(["www", "youtube", "com"])), .remote, "public name")
            try expectEqual(scope(dnsQuery(["nas", "lan"])), .local, ".lan")
            try expectEqual(scope(dnsQuery(["printer", "local"])), .local, ".local")
            try expectEqual(scope(dnsQuery(["router", "home", "arpa"])), .local, ".home.arpa")
            try expectEqual(scope(dnsQuery(["nas"])), .local, "single label")
            try expectEqual(
                scope(dnsQuery(["1", "0", "168", "192", "in-addr", "arpa"])),
                .local,
                "reverse lookup"
            )
            try expectEqual(scope(Data([0x12, 0x34])), nil, "truncated")
            var noQuestion = [UInt8](dnsQuery(["example", "com"]))
            noQuestion[5] = 0
            try expectEqual(scope(Data(noQuestion)), nil, "no question")
            var overrun = [UInt8](dnsQuery(["example", "com"]))
            overrun[12] = 40
            try expectEqual(scope(Data(overrun)), nil, "label overruns message")
        }

        run.test("dns_route_sends_public_queries_to_mihomo") {
            let router = SOCKS5Endpoint(address: SOCKS5Address(ipAddress: try IPAddress("192.168.0.1")), port: 53)
            let google = SOCKS5Endpoint(address: SOCKS5Address(ipAddress: try IPAddress("8.8.8.8")), port: 53)
            func route(
                _ destination: SOCKS5Endpoint,
                trusted: Bool = false,
                scope: DNSQueryScope?,
                mihomoAvailable: Bool = true
            ) -> DNSRelayRoute {
                DNSRelayRoutingPolicy.route(
                    destination: destination,
                    isTrustedMyproxyComponent: trusted,
                    queryScope: scope,
                    mihomoAvailable: mihomoAvailable
                )
            }
            try expectEqual(route(router, scope: .remote), .mihomo, "public name to router")
            try expectEqual(route(router, scope: .local), .directLocalResolver, "LAN name to router")
            try expectEqual(route(router, scope: nil), .directLocalResolver, "unparsed query to router")
            try expectEqual(route(google, scope: .remote), .mihomo, "public resolver")
            try expectEqual(route(google, scope: nil), .mihomo, "public resolver before first datagram")
            try expectEqual(route(google, scope: .native), .directNativeFlow, "DIRECT app lookup")
            try expectEqual(
                route(router, scope: .remote, mihomoAvailable: false),
                .directMihomoUnavailable,
                "Mihomo down"
            )
            try expectEqual(
                DNSRelayRoute.directMihomoUnavailable.target(for: router, resolvers: []),
                router,
                "Mihomo down keeps the addressed resolver"
            )
            try expectEqual(route(router, scope: .native), .directNativeFlow, "DIRECT app lookup via router")
            try expectEqual(
                route(google, trusted: true, scope: .remote),
                .directTrustedComponent,
                "Mihomo's own upstream lookup"
            )
            try expectEqual(
                route(SOCKS5Endpoint(address: try SOCKS5Address(domain: "nas.lan"), port: 53), scope: nil),
                .directLocalResolver,
                "LAN name endpoint"
            )
            try expectEqual(
                route(SOCKS5Endpoint(address: try SOCKS5Address(domain: "www.youtube.com"), port: 53), scope: nil),
                .mihomo,
                "public name endpoint"
            )
        }

        run.test("dns_route_target") {
            let router = SOCKS5Endpoint(address: SOCKS5Address(ipAddress: try IPAddress("192.168.0.1")), port: 53)
            let cloudflare = SOCKS5Endpoint(address: SOCKS5Address(ipAddress: try IPAddress("1.1.1.1")), port: 53)
            let mihomo = SOCKS5Endpoint(address: SOCKS5Address(ipAddress: try IPAddress("127.0.0.1")), port: 1053)
            let lanName = SOCKS5Endpoint(address: try SOCKS5Address(domain: "nas.lan"), port: 53)
            try expectEqual(DNSRelayRoute.mihomo.target(for: router, resolvers: [cloudflare]), mihomo, "Mihomo fake-ip DNS")
            try expectEqual(
                DNSRelayRoute.directLocalResolver.target(for: router, resolvers: [cloudflare]),
                router,
                "LAN resolver kept"
            )
            try expectEqual(
                DNSRelayRoute.directNativeFlow.target(for: router, resolvers: [cloudflare]),
                router,
                "DIRECT app keeps its resolver"
            )
            try expectEqual(
                DNSRelayRoute.directLocalResolver.target(for: lanName, resolvers: [cloudflare]),
                cloudflare,
                "name endpoint dials a resolver address"
            )
        }

        let total = run.passed + run.failed
        print("\(run.passed) passed, \(run.failed) failed, \(total) total")
        if run.failed > 0 {
            exit(1)
        }
    }
}
