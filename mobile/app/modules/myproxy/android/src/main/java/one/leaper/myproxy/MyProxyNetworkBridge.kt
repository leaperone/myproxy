package one.leaper.myproxy

import android.net.VpnService
import one.leaper.myproxy.core.NativeCore
import one.leaper.myproxy.network.mobile.Mobile
import one.leaper.myproxy.network.mobile.Engine
import one.leaper.myproxy.network.mobile.Policy
import one.leaper.myproxy.network.mobile.Protector

/** Typed adapter for the CI-generated myproxy-network.aar. */
internal class MyProxyNetworkBridge(
    private val service: VpnService,
    renderJSON: String
) {
    private val policy = object : Policy {
        override fun decide(requestJSON: String): String = NativeCore.request(requestJSON)
        override fun health(node: String, delayMs: Long, failed: Boolean) {
            NativeCore.request(org.json.JSONObject().put("op", "health").put("node", node).put("delayMs", delayMs).put("failed", failed).toString())
        }
    }
    private val protector = object : Protector {
        override fun protect(fd: Long): Boolean = service.protect(fd.toInt())
    }
    private val engine: Engine = Mobile.newEngine(renderJSON, policy, protector, true)

    init {
        val revision = org.json.JSONObject(renderJSON).getLong("revision")
        val result = org.json.JSONObject(NativeCore.request(org.json.JSONObject().put("op", "activate").put("revision", revision).toString()))
        if (!result.optBoolean("ok")) {
            engine.close()
            throw IllegalStateException("配置已发生变化，请重新连接")
        }
    }

    fun startTun(fd: Int) { engine.startTun(fd.toLong()) }
    fun close() { engine.close() }
    fun closeConnections() { engine.closeConnections() }
    fun probe() { engine.probe() }
    fun snapshot(): String = engine.snapshot()
}
