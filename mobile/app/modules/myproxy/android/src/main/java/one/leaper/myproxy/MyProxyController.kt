package one.leaper.myproxy

import android.content.Context
import one.leaper.myproxy.core.NativeCore
import org.json.JSONObject
import java.io.BufferedInputStream
import java.io.ByteArrayOutputStream
import java.net.HttpURLConnection
import java.net.URL
import java.nio.charset.StandardCharsets
import java.util.UUID
import java.util.concurrent.TimeUnit

internal class MyProxyController(private val context: Context) {
    private val store = MyProxyStore(context)
    private val lock = globalLock
    private var snapshot = "{\"ok\":false,\"error\":{\"code\":\"not_initialized\",\"message\":\"尚未初始化\"}}"

    fun request(json: String): String = synchronized(lock) {
        val request = try { JSONObject(json) } catch (_: Exception) {
            return@synchronized error("invalid_request", "请求格式不正确")
        }
        return@synchronized try {
            when (request.optString("op")) {
                "snapshot" -> mergeRuntime(native("{\"op\":\"snapshot\"}"))
                "connect" -> invokeRuntime("connect", request)
                "disconnect" -> invokeRuntime("disconnect", request)
                "probe" -> invokeRuntime("probe", request)
                "select", "setMode", "setAutoConnect", "setFallback", "saveRule", "deleteRule" ->
                    invokeRuntime(request.optString("op"), request)
                "import" -> importText(request)
                "addSource" -> refreshSource(request, true)
                "refreshSource" -> refreshSource(request, false)
                "removeSource" -> persistAfterNative(request)
                else -> error("unsupported_operation", "暂不支持这个操作")
            }.also { response ->
                if (JSONObject(response).optBoolean("ok")) snapshot = response
            }
        } catch (e: Exception) {
            error("request_failed", e.message ?: "操作失败")
        }
    }

    fun loadForRuntime(): String? = synchronized(lock) {
        store.read()?.also { document ->
            val loaded = JSONObject(native(JSONObject().put("op", "load").put("platform", "android").put("document", document).toString()))
            if (!loaded.optBoolean("ok")) return@synchronized null
        }
    }

    fun shouldAutoConnect(): Boolean = synchronized(lock) {
        if (store.read() == null || MyProxyVpnService.isActive()) return false
        runCatching {
            val data = JSONObject(native("{\"op\":\"snapshot\"}")).optJSONObject("data") ?: return false
            if (!data.optBoolean("autoConnect")) return false
            val nodes = data.optJSONArray("nodes") ?: return false
            for (index in 0 until nodes.length()) {
                if (nodes.optJSONObject(index)?.optBoolean("available", false) == true) return true
            }
            false
        }.getOrDefault(false)
    }

    private fun importText(r: JSONObject): String {
        val text = r.optString("text")
        if (text.isBlank() || text.length > 2_000_000) return error("invalid_import", "节点内容为空或过大")
        val req = JSONObject().put("op", "import").put("text", text)
        r.optString("name").takeIf { it.isNotBlank() }?.let { req.put("name", it) }
        return persistAfterNative(req)
    }

    private fun refreshSource(r: JSONObject, adding: Boolean): String {
        val source = r.optString("id").takeIf { it.isNotBlank() }?.let(::findSource)
        val url = r.optString("url").takeIf { it.isNotBlank() } ?: source?.first.orEmpty()
        if (!url.startsWith("https://") && !url.startsWith("http://")) return error("invalid_source", "订阅地址必须是 http 或 https")
        val text = boundedFetch(url)
        val req = JSONObject().put("op", "import").put("text", text).put("sourceURL", url)
        if (adding) req.put("sourceId", UUID.randomUUID().toString())
        r.optString("id").takeIf { it.isNotBlank() }?.let { req.put("sourceId", it) }
        r.optString("name").takeIf { it.isNotBlank() }?.let { req.put("name", it) }
        source?.second?.takeIf { r.optString("name").isBlank() }?.let { req.put("name", it) }
        return persistAfterNative(req)
    }

    private fun findSource(id: String): Pair<String, String>? = runCatching {
        val response = JSONObject(native("{\"op\":\"snapshot\"}"))
        val data = response.optJSONObject("data") ?: return null
        val sources = data.optJSONArray("sources") ?: return null
        for (index in 0 until sources.length()) {
            val source = sources.optJSONObject(index) ?: continue
            if (source.optString("id") == id) return source.optString("url") to source.optString("name")
        }
        null
    }.getOrNull()

    private fun persistAfterNative(request: JSONObject): String {
        val response = native(request.toString())
        val obj = JSONObject(response)
        if (!obj.optBoolean("ok")) return response
        val exported = JSONObject(native("{\"op\":\"export\"}"))
        if (!exported.optBoolean("ok")) return error("persist_failed", "配置验证通过，但保存失败")
        store.write(exported.optString("data"))
        return response
    }

    private fun invokeRuntime(op: String, r: JSONObject): String = persistAfterNative(JSONObject(r.toString()).put("op", op))

    private fun native(request: String): String = NativeCore.request(request)

    private fun mergeRuntime(response: String): String {
        val value = MyProxyVpnService.runtimeSnapshot()
            ?: "{\"phase\":\"disconnected\",\"message\":null,\"connectedAt\":null,\"uploadBytes\":0,\"downloadBytes\":0,\"connections\":[]}"
        return runCatching {
            val root = JSONObject(response); val data = root.optJSONObject("data") ?: return response
            data.put("runtime", JSONObject(value)); root.toString()
        }.getOrDefault(response)
    }

    private fun boundedFetch(url: String): String {
        val connection = URL(url).openConnection() as HttpURLConnection
        connection.connectTimeout = 10_000; connection.readTimeout = 15_000
        connection.instanceFollowRedirects = true
        return try {
            if (connection.responseCode !in 200..299) throw IllegalStateException("订阅服务器返回 ${connection.responseCode}")
            BufferedInputStream(connection.inputStream).use { input ->
            val out = ByteArrayOutputStream(); val buf = ByteArray(16 * 1024); var total = 0
            while (true) {
                val n = input.read(buf); if (n < 0) break
                total += n; if (total > 2_000_000) throw IllegalStateException("订阅内容超过 2 MB")
                out.write(buf, 0, n)
            }
            out.toString(StandardCharsets.UTF_8.name())
            }
        } finally { connection.disconnect() }
    }

    private fun error(code: String, message: String) = JSONObject()
        .put("ok", false).put("error", JSONObject().put("code", code).put("message", message)).toString()

    companion object { private val globalLock = Any() }
}
