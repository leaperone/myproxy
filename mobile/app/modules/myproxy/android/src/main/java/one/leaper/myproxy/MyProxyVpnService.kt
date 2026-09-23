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
        try { tunnel?.close() } catch (_: Exception) { }
        tunnel = null
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

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
        private const val CHANNEL = "myproxy-vpn"
    }
}
