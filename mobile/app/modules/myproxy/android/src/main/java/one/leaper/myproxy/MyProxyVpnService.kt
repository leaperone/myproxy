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
import org.json.JSONObject

class MyProxyVpnService : VpnService() {
    private var tunnel: ParcelFileDescriptor? = null
    private var bridge: MyProxyNetworkBridge? = null
    private var started = false

    override fun onCreate() {
        super.onCreate()
        createChannel()
        if (Build.VERSION.SDK_INT >= 29) {
            startForeground(4108, notification("正在启动 MyProxy"), ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
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
        return START_NOT_STICKY
    }

    private fun startIfNeeded() {
        if (started) return
        val controller = MyProxyController(applicationContext)
        val document = controller.loadForRuntime() ?: run { stopSelf(); return }
        val render = NativeCore.request("{\"op\":\"render\"}")
        val renderObj = JSONObject(render)
        if (!renderObj.optBoolean("ok")) { stopSelf(); return }
        try {
            val builder = Builder().setSession("MyProxy")
                .setMtu(1500).addAddress("198.18.0.1", 30).addRoute("0.0.0.0", 0)
            if (Build.VERSION.SDK_INT >= 21) builder.addAddress("fd00:1::1", 126).addRoute("::", 0)
            builder.addDnsServer("198.18.0.2")
            tunnel = builder.establish() ?: error("VpnService.Builder.establish failed")
            bridge = MyProxyNetworkBridge(this, renderObj.optString("data"))
            bridge!!.startTun(tunnel!!.fd)
            started = true
            writeRuntime(bridge!!.snapshot())
            updateNotification("MyProxy 已连接")
        } catch (_: Exception) {
            stopVpn()
        }
    }

    override fun onRevoke() { stopVpn() }

    override fun onDestroy() { stopVpn(); super.onDestroy() }

    private fun stopVpn() {
        started = false
        try { bridge?.close() } catch (_: Exception) { }
        bridge = null
        writeRuntime("{\"phase\":\"disconnected\",\"message\":null,\"connectedAt\":null,\"uploadBytes\":0,\"downloadBytes\":0,\"connections\":[]}")
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
            bridge = MyProxyNetworkBridge(this, render.optString("data")); bridge!!.startTun(tunnel?.fd ?: error("TUN closed"))
            writeRuntime(bridge!!.snapshot())
        } catch (_: Exception) { stopVpn() }
    }

    private fun writeRuntime(value: String) { MyProxyStore(applicationContext).writeRuntime(value) }

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
        private const val CHANNEL = "myproxy-vpn"
    }
}
