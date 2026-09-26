import Foundation

enum MyProxyNativeCore {
    private static let lock = NSLock()
    static func call(_ request: String) -> String {
        lock.lock(); defer { lock.unlock() }
        guard let input = request.cString(using: .utf8), let raw = myproxy_mobile_call(input) else {
            return "{\"ok\":false,\"error\":{\"code\":\"native_core_unavailable\",\"message\":\"移动核心尚未安装\"}}"
        }
        let out = String(cString: raw); myproxy_mobile_free(raw); return out
    }
}
@_silgen_name("myproxy_mobile_call") private func myproxy_mobile_call(_ request: UnsafePointer<CChar>) -> UnsafePointer<CChar>?
@_silgen_name("myproxy_mobile_free") private func myproxy_mobile_free(_ response: UnsafePointer<CChar>)
