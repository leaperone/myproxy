package one.leaper.myproxy

import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import android.content.Intent
import android.net.VpnService
import android.os.Build
import android.os.SystemClock
import org.json.JSONObject

class MyProxyModule : Module() {
    private val controller by lazy { MyProxyController(requireNotNull(appContext.reactContext)) }

    override fun definition() = ModuleDefinition {
        Name("MyProxy")
        AsyncFunction("request") { requestJSON: String ->
            withContext(Dispatchers.IO) {
                val context = requireNotNull(appContext.reactContext)
                val op = runCatching { JSONObject(requestJSON).optString("op") }.getOrNull()
                when (op) {
                    "connect" -> {
                        val permission = VpnService.prepare(context)
                        if (permission != null) {
                            appContext.currentActivity?.startActivityForResult(permission, VPN_PERMISSION_REQUEST_CODE)
                            "{\"ok\":false,\"error\":{\"code\":\"vpn_permission_required\",\"message\":\"请允许 MyProxy 使用 VPN\"}}"
                        } else {
                            val intent = Intent(context, MyProxyVpnService::class.java).setAction(MyProxyVpnService.ACTION_CONNECT)
                            if (Build.VERSION.SDK_INT >= 26) context.startForegroundService(intent) else context.startService(intent)
                            waitForRuntime() ?: JSONObject().put("ok", false).put("error", JSONObject().put("code", "vpn_start_timeout").put("message", "VPN 启动超时，请稍后重试")).toString()
                        }
                    }
                    "disconnect" -> {
                        context.stopService(Intent(context, MyProxyVpnService::class.java))
                        controller.request(requestJSON)
                    }
                    else -> {
                        val response = controller.request(requestJSON)
                        if (op in setOf("probe", "select", "setMode", "setAutoConnect", "setFallback", "saveRule", "deleteRule", "removeSource", "import", "addSource", "refreshSource")) {
                            if (MyProxyVpnService.isActive()) {
                                val action = if (op == "probe") MyProxyVpnService.ACTION_PROBE else MyProxyVpnService.ACTION_APPLY
                                runCatching { context.startService(Intent(context, MyProxyVpnService::class.java).setAction(action)) }
                                return@withContext waitForRuntime() ?: response
                            }
                        }
                        response
                    }
                }
            }
        }
    }

    private fun waitForRuntime(): String? {
        SystemClock.sleep(100)
        repeat(120) {
            if (MyProxyVpnService.runtimeSnapshot() != null) return controller.request("{\"op\":\"snapshot\"}")
            SystemClock.sleep(50)
        }
        return null
    }

    companion object { private const val VPN_PERMISSION_REQUEST_CODE = 4109 }
}
