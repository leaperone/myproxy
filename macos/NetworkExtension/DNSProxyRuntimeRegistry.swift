import Foundation
import MyproxyNetworkShared

enum DNSProxyRuntimeRegistryError: Error, Equatable, LocalizedError, Sendable {
    case activationMismatch
    case bootstrapUnavailable
    case deliveredBootstrapMismatch

    var errorDescription: String? {
        switch self {
        case .activationMismatch:
            "DNS runtime publication does not match the prepared activation."
        case .bootstrapUnavailable:
            "No DNS bootstrap was staged by the host or delivered by macOS."
        case .deliveredBootstrapMismatch:
            "The DNS bootstrap delivered by macOS does not match the host-staged activation."
        }
    }
}

/// Process-local bridge between the DNS provider and the transparent provider.
///
/// Apple hosts all providers declared by one Network Extension system
/// extension in the same root-owned process. The transparent provider already
/// exposes an authenticated `sendProviderMessage` channel to the containing
/// app, so DNS status can cross the privilege boundary without a per-user App
/// Group file or a world-readable root-owned artifact.
final class DNSProxyRuntimeRegistry: @unchecked Sendable {
    typealias LiveUpdater = @Sendable (DNSProxyBootstrapConfiguration) -> Bool

    static let shared = DNSProxyRuntimeRegistry()

    private let lock = NSLock()
    private var expectedRevision: UInt64?
    private var expectedActivationIdentifier: UUID?
    private var bootstrap: DNSProxyBootstrapConfiguration?
    private var status: DNSProxyRuntimeStatus?
    private var startupFailure: DNSProxyStartupFailure?
    private var liveUpdater: (id: UUID, apply: LiveUpdater)?

    init() {}

    @discardableResult
    func prepare(_ value: DNSProxyBootstrapConfiguration) -> Bool {
        guard (try? value.validate()) != nil else { return false }
        lock.lock()
        if bootstrap == value,
           expectedRevision == value.revision,
           expectedActivationIdentifier == value.activationIdentifier {
            lock.unlock()
            return true
        }
        let previousExpectedRevision = expectedRevision
        let previousExpectedActivationIdentifier = expectedActivationIdentifier
        let previousBootstrap = bootstrap
        let previousStatus = status
        let previousStartupFailure = startupFailure
        expectedRevision = value.revision
        expectedActivationIdentifier = value.activationIdentifier
        bootstrap = value
        status = nil
        startupFailure = nil
        let updater = liveUpdater?.apply
        lock.unlock()
        guard updater?(value) ?? true else {
            lock.lock()
            // The live updater publishes status through this registry, so the
            // candidate identity must be staged before invoking it. If the
            // provider rejects the candidate, restore the entire previous
            // activation so an identical retry cannot hit the idempotent path
            // while the provider is still running the old data plane.
            if bootstrap == value,
               expectedRevision == value.revision,
               expectedActivationIdentifier == value.activationIdentifier {
                expectedRevision = previousExpectedRevision
                expectedActivationIdentifier = previousExpectedActivationIdentifier
                bootstrap = previousBootstrap
                status = previousStatus
                startupFailure = previousStartupFailure
            }
            lock.unlock()
            return false
        }
        return true
    }

    func registerLiveUpdater(_ updater: @escaping LiveUpdater) -> UUID {
        let id = UUID()
        lock.lock()
        liveUpdater = (id, updater)
        lock.unlock()
        return id
    }

    func unregisterLiveUpdater(_ id: UUID) {
        lock.lock()
        if liveUpdater?.id == id {
            liveUpdater = nil
        }
        lock.unlock()
    }

    /// Resolves one start attempt without a read/replace race. A host-staged
    /// bootstrap is primary; macOS-delivered options may confirm it but never
    /// overwrite a different prepared activation.
    func resolveBootstrap(
        delivered: DNSProxyBootstrapConfiguration?
    ) throws -> DNSProxyBootstrapConfiguration {
        lock.lock()
        defer { lock.unlock() }
        let prepared = bootstrap
        do {
            let selected = try DNSProxyBootstrapConfiguration.resolve(
                prepared: prepared,
                delivered: delivered
            )
            if prepared == nil {
                expectedRevision = selected.revision
                expectedActivationIdentifier = selected.activationIdentifier
                bootstrap = selected
                status = nil
                startupFailure = nil
            }
            return selected
        } catch DNSProxyBootstrapResolutionError.deliveredBootstrapMismatch {
            if prepared != nil {
                status = nil
                startupFailure = DNSProxyStartupFailure(
                    reason: .invalidBootstrapPayload
                )
            }
            throw DNSProxyRuntimeRegistryError.deliveredBootstrapMismatch
        } catch {
            throw DNSProxyRuntimeRegistryError.bootstrapUnavailable
        }
    }

    func publish(_ value: DNSProxyRuntimeStatus) throws {
        try value.validate()
        lock.lock()
        defer { lock.unlock() }

        if expectedRevision == nil || expectedActivationIdentifier == nil {
            expectedRevision = value.revision
            expectedActivationIdentifier = value.activationIdentifier
        }
        guard expectedRevision == value.revision,
              expectedActivationIdentifier == value.activationIdentifier
        else {
            throw DNSProxyRuntimeRegistryError.activationMismatch
        }
        status = value
        startupFailure = nil
    }

    func publishStartupFailure(
        _ reason: DNSProxyStartupFailureReason,
        for bootstrap: DNSProxyBootstrapConfiguration
    ) {
        lock.lock()
        defer { lock.unlock() }
        guard expectedRevision == bootstrap.revision,
              expectedActivationIdentifier == bootstrap.activationIdentifier
        else {
            return
        }
        status = nil
        startupFailure = DNSProxyStartupFailure(reason: reason)
    }

    func snapshot() -> DNSProxyRuntimeReport? {
        lock.lock()
        defer { lock.unlock() }
        guard let expectedRevision, let expectedActivationIdentifier else {
            return nil
        }
        return DNSProxyRuntimeReport(
            expectedRevision: expectedRevision,
            expectedActivationIdentifier: expectedActivationIdentifier,
            status: status,
            startupFailure: startupFailure
        )
    }
}
