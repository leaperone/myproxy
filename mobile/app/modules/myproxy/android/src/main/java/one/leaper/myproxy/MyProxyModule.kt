package one.leaper.myproxy

import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import android.content.Intent
import android.net.VpnService
import android.os.Build
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
                            appContext.currentActivity?.startActivity(permission)
                            "{\"ok\":false,\"error\":{\"code\":\"vpn_permission_required\",\"message\":\"请允许 MyProxy 使用 VPN\"}}"
                        } else {
                            val intent = Intent(context, MyProxyVpnService::class.java).setAction(MyProxyVpnService.ACTION_CONNECT)
                            if (Build.VERSION.SDK_INT >= 26) context.startForegroundService(intent) else context.startService(intent)
                            NativeCore.request("{\"op\":\"snapshot\"}")
                        }
                    }
                    "disconnect" -> {
                        context.stopService(Intent(context, MyProxyVpnService::class.java))
                        controller.request(requestJSON)
                    }
                    else -> controller.request(requestJSON)
                }
            }
        }
    }
}
