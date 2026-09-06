import Foundation

enum NetworkExtensionControlOperation: String, Sendable {
    case activateSystemExtension
    case configureTransparentProxy
    case startTransparentProxy
    case stopTransparentProxy
    case configureDNSProxy
    case stopDNSProxy
}

struct NetworkExtensionControlFailure: Error, Sendable, LocalizedError {
    let operation: NetworkExtensionControlOperation
    let message: String

    init(operation: NetworkExtensionControlOperation, message: String) {
        self.operation = operation
        self.message = message
    }

    init(operation: NetworkExtensionControlOperation, underlying error: Error) {
        if let failure = error as? NetworkExtensionControlFailure {
            self.init(operation: operation, message: failure.message)
            return
        }
        let underlyingError = error as NSError
        var message = underlyingError.localizedDescription
        if underlyingError.domain == "OSSystemExtensionErrorDomain",
           underlyingError.code == 9 {
            message =
                "macOS rejected the Network Extension package during validation. Install a Developer ID build that embeds the system extension"
        }
        if underlyingError.domain != NSCocoaErrorDomain {
            message += " (\(underlyingError.domain) \(underlyingError.code))"
        }
        self.init(operation: operation, message: message)
    }

    var errorDescription: String? { "\(operation.rawValue): \(message)" }
}

enum SystemExtensionRequestProgress: Equatable, Sendable {
    case awaitingUserApproval
}

enum SystemExtensionRequestOutcome: Equatable, Sendable {
    case completed
    case requiresReboot
}
