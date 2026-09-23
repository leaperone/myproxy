package one.leaper.myproxy

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Intent
import android.net.VpnService
import android.os.Build
import android.os.ParcelFileDescriptor
import android.content.pm.ServiceInfo
import androidx.core.app.NotificationCompat
import one.leaper.myproxy.core.NativeCore
import org.json.JSONObject

class MyProxyVpnService : VpnService() {
    private var tunnel: ParcelFileDescriptor? = null
    private var bridge: MyProxyNetworkBridge? = null
    private var started = false
    private var preserveError = false

    override fun onCreate() {
        super.onCreate()
        createChannel()
        if (Build.VERSION.SDK_INT >= 34) {
            startForeground(4108, notification("正在启动 MyProxy"), ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        } else if (Build.VERSION.SDK_INT >= 29) {
            startForeground(4108, notification("正在启动 MyProxy"), ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        } else {
            startForeground(4108, notification("正在启动 MyProxy"))
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        // A null intent or SERVICE_INTERFACE is how Always-on VPN may restart us.
        if (intent == null || intent.action == SERVICE_INTERFACE || intent.action == ACTION_CONNECT) startIfNeeded()
        if (intent?.action == ACTION_APPLY) { if (started) applyCurrent() else startIfNeeded() }
        if (intent?.action == ACTION_PROBE) { bridge?.probe(); bridge?.snapshot()?.let(::writeRuntime) }
        if (intent?.action == ACTION_DISCONNECT) stopVpn()
        return START_STICKY
    }

    private fun startIfNeeded() {
        if (started) return
        preserveError = false
        val controller = MyProxyController(applicationContext)
        if (controller.loadForRuntime() == null) { failRuntime("还没有可用的代理配置"); return }
        val render = NativeCore.request("{\"op\":\"render\"}")
        val renderObj = JSONObject(render)
        if (!renderObj.optBoolean("ok")) { failRuntime("代理配置无法启动"); return }
        try {
            val builder = Builder().setSession("MyProxy")
                .setMtu(1500).addAddress("198.18.0.1", 30).addRoute("0.0.0.0", 0)
            if (Build.VERSION.SDK_INT >= 21) builder.addAddress("fd00:1::1", 126).addRoute("::", 0)
            builder.addDnsServer("1.1.1.1").addDnsServer("9.9.9.9")
            tunnel = builder.establish() ?: error("VpnService.Builder.establish failed")
            val renderData = renderObj.optJSONObject("data")?.toString() ?: renderObj.optString("data")
            bridge = MyProxyNetworkBridge(this, renderData)
            bridge!!.startTun(tunnel!!.fd)
            started = true
            liveActive = true
            writeRuntime(bridge!!.snapshot())
            updateNotification("MyProxy 已连接")
        } catch (error: Exception) {
            failRuntime(error.message ?: "VPN 启动失败")
        }
    }

    override fun onRevoke() { stopVpn() }

    override fun onDestroy() { stopVpn(); super.onDestroy() }

    private fun stopVpn(writeDisconnected: Boolean = true) {
        started = false
        liveActive = false
        try { bridge?.close() } catch (_: Exception) { }
        bridge = null
        if (writeDisconnected && !preserveError) writeRuntime("{\"phase\":\"disconnected\",\"message\":null,\"connectedAt\":null,\"uploadBytes\":0,\"downloadBytes\":0,\"connections\":[]}")
        if (!writeDisconnected) preserveError = true
        try { tunnel?.close() } catch (_: Exception) { }
        tunnel = null
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    private fun applyCurrent() {
        try {
            bridge?.close(); bridge = null
            val render = JSONObject(NativeCore.request("{\"op\":\"render\"}"))
            if (!render.optBoolean("ok")) throw IllegalStateException("render failed")
            val renderData = render.optJSONObject("data")?.toString() ?: render.optString("data")
            bridge = MyProxyNetworkBridge(this, renderData); bridge!!.startTun(tunnel?.fd ?: error("TUN closed"))
            writeRuntime(bridge!!.snapshot())
        } catch (error: Exception) { failRuntime(error.message ?: "应用配置失败") }
    }

    private fun failRuntime(message: String) {
        preserveError = true
        writeRuntime("{\"phase\":\"error\",\"message\":${JSONObject.quote(message)},\"connectedAt\":null,\"uploadBytes\":0,\"downloadBytes\":0,\"connections\":[]}")
        stopVpn(writeDisconnected = false)
    }

    private fun writeRuntime(value: String) { liveRuntime = value; MyProxyStore(applicationContext).writeRuntime(value) }

    private fun createChannel() {
        if (Build.VERSION.SDK_INT >= 26) getSystemService(NotificationManager::class.java)
            .createNotificationChannel(NotificationChannel(CHANNEL, "MyProxy VPN", NotificationManager.IMPORTANCE_LOW))
    }
    private fun notification(text: String): Notification = NotificationCompat.Builder(this, CHANNEL)
        .setSmallIcon(android.R.drawable.stat_sys_warning).setContentTitle("MyProxy").setContentText(text)
        .setOngoing(true).build()
    private fun updateNotification(text: String) { getSystemService(NotificationManager::class.java).notify(4108, notification(text)) }

    companion object {
        const val ACTION_CONNECT = "one.leaper.myproxy.action.CONNECT"
        const val ACTION_DISCONNECT = "one.leaper.myproxy.action.DISCONNECT"
        const val ACTION_APPLY = "one.leaper.myproxy.action.APPLY"
        const val ACTION_PROBE = "one.leaper.myproxy.action.PROBE"
        @Volatile private var liveRuntime: String? = null
        @Volatile private var liveActive: Boolean = false
        fun runtimeSnapshot(): String? = if (liveActive || liveRuntime?.contains("\"phase\":\"error\"") == true) liveRuntime else null
        fun isActive(): Boolean = liveActive
        private const val CHANNEL = "myproxy-vpn"
    }
}
