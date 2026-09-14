import Foundation

/// Deadlines for Host-owned Network Extension callbacks.
enum OnceReplyTimeout {
    static let providerMessage: Duration = .seconds(3)
    static let preferences: Duration = .seconds(5)
    static let supersededOperation: Duration = .seconds(3)
}

/// Resolves a checked continuation at most once.
/// Safe to finish from an NE callback, a timeout, and a cancellation handler.
final class OnceReply<Success: Sendable>: @unchecked Sendable {
    private let lock = NSLock()
    private var continuation: CheckedContinuation<Success, Error>?
    private var result: Result<Success, Error>?

    func attach(_ continuation: CheckedContinuation<Success, Error>) {
        lock.lock()
        if let result {
            lock.unlock()
            continuation.resume(with: result)
        } else {
            self.continuation = continuation
            lock.unlock()
        }
    }

    func finish(_ result: Result<Success, Error>) {
        lock.lock()
        guard self.result == nil else {
            lock.unlock()
            return
        }
        self.result = result
        let continuation = self.continuation
        self.continuation = nil
        lock.unlock()
        continuation?.resume(with: result)
    }
}

/// Run an NE-style callback so it cannot leak a continuation.
func awaitOnceReply<Success: Sendable>(
    timeout: Duration,
    operation: NetworkExtensionControlOperation,
    timeoutMessage: String,
    work: (OnceReply<Success>) -> Void
) async throws -> Success {
    let reply = OnceReply<Success>()
    return try await withTaskCancellationHandler {
        try await withCheckedThrowingContinuation { continuation in
            reply.attach(continuation)
            guard !Task.isCancelled else {
                reply.finish(.failure(CancellationError()))
                return
            }
            work(reply)
            let parts = timeout.components
            let seconds = TimeInterval(parts.seconds)
                + TimeInterval(parts.attoseconds) / 1_000_000_000_000_000_000
            DispatchQueue.global().asyncAfter(deadline: .now() + seconds) {
                reply.finish(
                    .failure(
                        NetworkExtensionControlFailure(
                            operation: operation,
                            message: timeoutMessage
                        )
                    )
                )
            }
        }
    } onCancel: {
        reply.finish(.failure(CancellationError()))
    }
}

/// Void preference APIs such as load/save/removeFromPreferences.
func awaitPreferenceCallback(
    operation: NetworkExtensionControlOperation,
    api: String,
    timeout: Duration = OnceReplyTimeout.preferences,
    start: (@escaping (@escaping @Sendable (Error?) -> Void) -> Void)
) async throws {
    try await awaitOnceReply(
        timeout: timeout,
        operation: operation,
        timeoutMessage: "Timed out waiting for \(api)"
    ) { (reply: OnceReply<Void>) in
        start { error in
            if let error {
                reply.finish(.failure(error))
            } else {
                reply.finish(.success(()))
            }
        }
    }
}
