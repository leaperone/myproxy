package one.leaper.myproxy

import android.net.VpnService
import one.leaper.myproxy.network.mobile.Mobile
import one.leaper.myproxy.network.mobile.MobileEngine
import one.leaper.myproxy.network.mobile.MobilePolicy
import one.leaper.myproxy.network.mobile.MobileProtector

/** Typed adapter for the CI-generated myproxy-network.aar. */
internal class MyProxyNetworkBridge(
    private val service: VpnService,
    renderJSON: String
) {
    private val policy = object : MobilePolicy {
        override fun decide(requestJSON: String): String = NativeCore.request(requestJSON)
        override fun health(node: String, delayMs: Long, failed: Boolean) {
            NativeCore.request("{\"op\":\"health\",\"node\":\"${node.replace("\"", "")}\",\"delayMs\":$delayMs,\"failed\":$failed}")
        }
    }
    private val protector = object : MobileProtector {
        override fun protect(fd: Long): Boolean = service.protect(fd.toInt())
    }
    private val engine: MobileEngine = Mobile.newEngine(renderJSON, policy, protector, true)

    fun startTun(fd: Int) { engine.startTun(fd.toLong()) }
    fun close() { engine.close() }
    fun closeConnections() { engine.closeConnections() }
    fun probe() { engine.probe() }
    fun snapshot(): String = engine.snapshot()
}
