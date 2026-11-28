import Foundation
import MyproxyNetworkShared

struct ProviderControlResponse: Codable, Sendable {
    let protocolVersion: Int
    let accepted: Bool
    let provider: String
    let revision: UInt64
    let running: Bool
    let captureEnabled: Bool
    let failOpen: Bool
    let message: String?
    let activityBatch: AppRoutingActivityBatch?
    let dnsRuntimeReport: DNSProxyRuntimeReport?
}

enum ProviderControlCodec {
    static func decode(_ data: Data) throws -> ProviderControlRequest {
        try JSONDecoder().decode(ProviderControlRequest.self, from: data)
    }

    static func encode(_ response: ProviderControlResponse) -> Data? {
        try? JSONEncoder().encode(response)
    }
}
