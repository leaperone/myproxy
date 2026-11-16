import Foundation

/// One idle deadline per relay or session. The data path only records
/// activity; the timer re-arms for the remaining interval when it fires, so a
/// busy relay does not schedule a dispatch work item for every chunk.
///
/// Every method must run on `queue`.
final class RelayIdleTimer: @unchecked Sendable {
    private let queue: DispatchQueue
    private let timeoutNanoseconds: UInt64
    private let onTimeout: () -> Void
    private var timer: DispatchSourceTimer?
    private var lastActivity: UInt64 = 0
    private var stopped = false

    init(
        queue: DispatchQueue,
        timeoutSeconds: Int,
        onTimeout: @escaping () -> Void
    ) {
        self.queue = queue
        timeoutNanoseconds = UInt64(max(timeoutSeconds, 1)) * 1_000_000_000
        self.onTimeout = onTimeout
    }

    func touch() {
        lastActivity = DispatchTime.now().uptimeNanoseconds
        guard !stopped, timer == nil else { return }
        let timer = DispatchSource.makeTimerSource(queue: queue)
        timer.setEventHandler { [weak self] in
            self?.fire()
        }
        timer.schedule(deadline: .now() + .nanoseconds(Int(timeoutNanoseconds)))
        self.timer = timer
        timer.resume()
    }

    func cancel() {
        stopped = true
        timer?.cancel()
        timer = nil
    }

    private func fire() {
        guard let timer else { return }
        let elapsed = DispatchTime.now().uptimeNanoseconds &- lastActivity
        guard elapsed >= timeoutNanoseconds else {
            let remaining = timeoutNanoseconds - elapsed
            timer.schedule(deadline: .now() + .nanoseconds(Int(remaining)))
            return
        }
        cancel()
        onTimeout()
    }
}
