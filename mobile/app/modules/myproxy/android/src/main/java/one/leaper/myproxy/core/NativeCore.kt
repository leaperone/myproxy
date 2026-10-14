package one.leaper.myproxy.core

/** JNI facade required by the mobile ABI. Rust owns all policy and validation. */
object NativeCore {
    init { try { System.loadLibrary("myproxy_mobile") } catch (_: UnsatisfiedLinkError) { } }
    @JvmStatic external fun call(request: String): String
    @JvmStatic fun request(request: String): String = try {
        call(request)
    } catch (_: UnsatisfiedLinkError) {
        "{\"ok\":false,\"error\":{\"code\":\"native_core_unavailable\",\"message\":\"移动核心尚未安装\"}}"
    }
}
