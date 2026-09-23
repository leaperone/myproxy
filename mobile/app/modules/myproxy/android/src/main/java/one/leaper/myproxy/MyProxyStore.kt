package one.leaper.myproxy

import android.content.Context
import java.io.File
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import java.nio.file.StandardCopyOption

internal class MyProxyStore(private val context: Context) {
    private val file get() = File(context.filesDir, "myproxy-mobile.document.json")
    private val temp get() = File(context.filesDir, "myproxy-mobile.document.json.tmp")

    @Synchronized fun read(): String? = file.takeIf { it.isFile }?.readText(StandardCharsets.UTF_8)

    @Synchronized fun write(document: String) {
        temp.writeText(document, StandardCharsets.UTF_8)
        Files.move(temp.toPath(), file.toPath(), StandardCopyOption.REPLACE_EXISTING, StandardCopyOption.ATOMIC_MOVE)
    }
}
