@preconcurrency import Foundation
@preconcurrency import SystemExtensions

protocol SystemExtensionControlling: Sendable {
    func activate() async throws -> SystemExtensionRequestOutcome
    func deactivate() async throws -> SystemExtensionRequestOutcome
}

final class AppleSystemExtensionController: SystemExtensionControlling, @unchecked Sendable {
    private let extensionIdentifier: String

    init(extensionIdentifier: String = MyproxyNetworkExtensionIdentifiers.systemExtension) {
        self.extensionIdentifier = extensionIdentifier
    }

    func activate() async throws -> SystemExtensionRequestOutcome {
        try await SystemExtensionRequestRunner(
            kind: .activation,
            extensionIdentifier: extensionIdentifier
        ).run()
    }

    func deactivate() async throws -> SystemExtensionRequestOutcome {
        try await SystemExtensionRequestRunner(
            kind: .deactivation,
            extensionIdentifier: extensionIdentifier
        ).run()
    }
}

private final class SystemExtensionRequestRunner: NSObject,
    OSSystemExtensionRequestDelegate,
    @unchecked Sendable
{
    enum Kind {
        case activation
        case deactivation
    }

    private let request: OSSystemExtensionRequest
    private let lock = NSLock()
    private var continuation: CheckedContinuation<SystemExtensionRequestOutcome, Error>?
    private var didFinish = false

    init(kind: Kind, extensionIdentifier: String) {
        let queue = DispatchQueue(label: "local.harry.myproxy.system-extension-request")
        switch kind {
        case .activation:
            request = OSSystemExtensionRequest.activationRequest(
                forExtensionWithIdentifier: extensionIdentifier,
                queue: queue
            )
        case .deactivation:
            request = OSSystemExtensionRequest.deactivationRequest(
                forExtensionWithIdentifier: extensionIdentifier,
                queue: queue
            )
        }
        super.init()
        request.delegate = self
    }

    func run() async throws -> SystemExtensionRequestOutcome {
        try await withCheckedThrowingContinuation { continuation in
            lock.lock()
            self.continuation = continuation
            lock.unlock()
            OSSystemExtensionManager.shared.submitRequest(request)
        }
    }

    func request(
        _ request: OSSystemExtensionRequest,
        actionForReplacingExtension existing: OSSystemExtensionProperties,
        withExtension ext: OSSystemExtensionProperties
    ) -> OSSystemExtensionRequest.ReplacementAction {
        .replace
    }

    func requestNeedsUserApproval(_ request: OSSystemExtensionRequest) {}

    func request(
        _ request: OSSystemExtensionRequest,
        didFinishWithResult result: OSSystemExtensionRequest.Result
    ) {
        switch result {
        case .completed:
            finish(.success(.completed))
        case .willCompleteAfterReboot:
            finish(.success(.requiresReboot))
        @unknown default:
            finish(
                .failure(
                    NetworkExtensionControlFailure(
                        operation: .activateSystemExtension,
                        message: "Unknown system extension request result"
                    )
                )
            )
        }
    }

    func request(_ request: OSSystemExtensionRequest, didFailWithError error: Error) {
        finish(
            .failure(
                NetworkExtensionControlFailure(
                    operation: .activateSystemExtension,
                    underlying: error
                )
            )
        )
    }

    private func finish(_ result: Result<SystemExtensionRequestOutcome, Error>) {
        let continuation: CheckedContinuation<SystemExtensionRequestOutcome, Error>?
        lock.lock()
        if didFinish {
            continuation = nil
        } else {
            didFinish = true
            continuation = self.continuation
            self.continuation = nil
        }
        lock.unlock()
        continuation?.resume(with: result)
    }
}
