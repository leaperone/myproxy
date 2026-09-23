package one.leaper.myproxy

import android.net.VpnService
import java.lang.reflect.Proxy

/**
 * Adapter for the gomobile ABI. Reflection keeps this module buildable before
 * the CI-generated AAR is present; a missing generated ABI is a hard runtime
 * error, never a simulated connected state.
 */
internal class MyProxyNetworkBridge(
    private val service: VpnService,
    private val renderJSON: String
) {
    private var engine: Any? = null

    fun startTun(fd: Int) {
        val mobile = Class.forName("one.leaper.myproxy.network.mobile.Mobile")
        val policyClass = Class.forName("one.leaper.myproxy.network.mobile.Policy")
        val protectorClass = Class.forName("one.leaper.myproxy.network.mobile.Protector")
        val policy = Proxy.newProxyInstance(policyClass.classLoader, arrayOf(policyClass)) { _, method, args ->
            if (method.name == "decide") NativeCore.request(args?.firstOrNull()?.toString() ?: "{}") else null
        }
        val protector = Proxy.newProxyInstance(protectorClass.classLoader, arrayOf(protectorClass)) { _, method, args ->
            if (method.name == "protect") service.protect((args?.firstOrNull() as Number).toInt()) else null
        }
        val factory = mobile.methods.firstOrNull { it.name == "newEngine" && it.parameterTypes.size == 4 }
            ?: error("gomobile Engine factory is unavailable")
        engine = factory.invoke(null, renderJSON, policy, protector, true)
        val start = engine!!.javaClass.methods.firstOrNull { it.name == "startTun" && it.parameterTypes.size == 1 }
            ?: error("gomobile startTun is unavailable")
        start.invoke(engine, fd.toLong())
    }

    fun close() {
        val current = engine ?: return
        current.javaClass.methods.firstOrNull { it.name == "close" && it.parameterTypes.isEmpty() }?.invoke(current)
        engine = null
    }

    fun snapshot(): String = engine?.javaClass?.methods
        ?.firstOrNull { it.name == "snapshot" && it.parameterTypes.isEmpty() }
        ?.invoke(engine)?.toString() ?: "{\"phase\":\"disconnected\"}"
}
