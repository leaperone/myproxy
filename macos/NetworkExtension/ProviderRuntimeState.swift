import Foundation

/// Small, lock-protected lifecycle state shared by provider callbacks.
/// NetworkExtension may invoke callbacks on different queues, so the state must
/// not rely on actor isolation or on a particular callback queue.
final class ProviderRuntimeState: @unchecked Sendable {
    private struct Storage {
        var revision: UInt64 = 0
        var running = false
        var captureEnabled = false
        var failOpen = true
        var pendingRevision: UInt64?
    }

    private let providerName: String
    private let lock = NSLock()
    private var storage = Storage()

    init(providerName: String) {
        self.providerName = providerName
    }

    func start(configuration: [String: Any]?) {
        withLock {
            storage.running = true
            // Every provider process starts in fail-open/quiesced mode. A
            // missing, malformed, or stale bootstrap configuration must never
            // enable capture accidentally.
            storage.captureEnabled = false
            storage.failOpen = true
            storage.pendingRevision = nil

            guard let configuration,
                  let revision = Self.uint64(configuration[ProviderConfigurationKey.revision])
            else {
                return
            }

            storage.revision = revision
            storage.captureEnabled = Self.bool(
                configuration[ProviderConfigurationKey.captureEnabled]
            ) ?? false
            storage.failOpen = Self.bool(
                configuration[ProviderConfigurationKey.failOpen]
            ) ?? true
        }
    }

    func stop() {
        withLock {
            storage.running = false
            storage.captureEnabled = false
            storage.failOpen = true
            storage.pendingRevision = nil
        }
    }

    func replace(
        revision: UInt64,
        captureEnabled: Bool,
        failOpen: Bool
    ) {
        withLock {
            guard storage.running, revision > storage.revision else { return }
            storage.revision = revision
            storage.captureEnabled = captureEnabled
            storage.failOpen = failOpen
            storage.pendingRevision = nil
        }
    }

    func apply(_ request: ProviderControlRequest) -> ProviderControlResponse {
        withLock {
            guard request.protocolVersion == ProviderControlRequest.currentProtocolVersion else {
                return response(
                    accepted: false,
                    message: "Unsupported provider control protocol version"
                )
            }
            switch request.command {
            case .status, .prepareDNS, .dnsStatus, .activity, .clearActivity:
                return response(accepted: true, message: nil)

            case .quiesce:
                guard let revision = request.revision,
                      revision > storage.revision
                else {
                    return response(
                        accepted: false,
                        message: "quiesce requires a revision newer than the active revision"
                    )
                }
                storage.captureEnabled = false
                storage.failOpen = true
                storage.revision = revision
                storage.pendingRevision = revision
                return response(accepted: true, message: "Provider is quiesced")

            case .applyConfiguration:
                guard let revision = request.revision else {
                    return response(
                        accepted: false,
                        message: "applyConfiguration requires a revision"
                    )
                }
                guard storage.captureEnabled == false,
                      storage.pendingRevision == revision,
                      revision == storage.revision
                else {
                    return response(
                        accepted: false,
                        message: "Configuration was not preceded by a matching quiesce"
                    )
                }

                storage.revision = revision
                storage.captureEnabled = request.captureEnabled ?? false
                storage.failOpen = request.failOpen ?? true
                storage.pendingRevision = nil
                return response(accepted: true, message: nil)
            }
        }
    }

    func snapshot(message: String? = nil) -> ProviderControlResponse {
        withLock {
            response(accepted: true, message: message)
        }
    }

    private func response(accepted: Bool, message: String?) -> ProviderControlResponse {
        ProviderControlResponse(
            protocolVersion: ProviderControlRequest.currentProtocolVersion,
            accepted: accepted,
            provider: providerName,
            revision: storage.revision,
            running: storage.running,
            captureEnabled: storage.captureEnabled,
            failOpen: storage.failOpen,
            message: message,
            activityBatch: nil,
            dnsRuntimeReport: nil
        )
    }

    private func withLock<T>(_ body: () -> T) -> T {
        lock.lock()
        defer { lock.unlock() }
        return body()
    }

    private static func uint64(_ value: Any?) -> UInt64? {
        switch value {
        case let value as UInt64:
            return value
        case let value as Int where value >= 0:
            return UInt64(value)
        case let value as NSNumber where value.int64Value >= 0:
            return value.uint64Value
        case let value as String:
            return UInt64(value)
        default:
            return nil
        }
    }

    private static func bool(_ value: Any?) -> Bool? {
        switch value {
        case let value as Bool:
            return value
        case let value as NSNumber:
            return value.boolValue
        case let value as String:
            switch value.lowercased() {
            case "true", "yes", "1": return true
            case "false", "no", "0": return false
            default: return nil
            }
        default:
            return nil
        }
    }
}
