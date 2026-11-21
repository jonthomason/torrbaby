package com.torrbaby.tv

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import java.io.FileOutputStream
import java.io.IOException
import java.net.HttpURLConnection
import java.net.URL

/**
 * Fetches the queued downloads one after another while the phone is otherwise idle, with a
 * notification showing progress (Android's rule for long transfers). A transfer continues from
 * the bytes already kept, with a Range request.
 */
class SavedService : Service() {
    private val main = Handler(Looper.getMainLooper())
    @Volatile private var running = false

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(NotificationChannel(CHANNEL, L10n.t("Downloads"), NotificationManager.IMPORTANCE_LOW))
        val notice = notice(L10n.t("Preparing…"), 0)
        if (Build.VERSION.SDK_INT >= 29) startForeground(NOTICE, notice, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC) else startForeground(NOTICE, notice)
        if (!running) { running = true; Thread({ work() }, "Downloads").start() }
        return START_NOT_STICKY
    }

    /** Android ends long data transfers after some hours a day; what is left waits for the next opening. */
    override fun onTimeout(startId: Int, reason: Int) { running = false; stopSelf() }

    private fun notice(text: String, percent: Int): Notification {
        val open = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE)
        return Notification.Builder(this, CHANNEL).setSmallIcon(android.R.drawable.stat_sys_download).setContentTitle(text)
            .setProgress(100, percent, percent == 0).setOngoing(true).setContentIntent(open).build()
    }

    // Compose's state lists may be read from any thread.
    private fun next(): SavedItem? = Saved.items.toList().firstOrNull { it.state == "queued" }

    private fun work() {
        try {
            while (running) {
                val item = next() ?: break
                if (!Saved.mayUseNetwork()) break
                fetch(item)
            }
        } finally {
            running = false
            main.post { Saved.live = null; stopForeground(STOP_FOREGROUND_REMOVE); stopSelf() }
        }
    }

    private fun fetch(item: SavedItem) {
        val api = Phone.api ?: run { main.post { Saved.update(item.id) { it.copy(state = "waiting") } }; return }
        main.post { Saved.update(item.id) { it.copy(state = "downloading", error = "") } }
        val partial = Saved.partial(item)
        var have = if (partial.exists()) partial.length() else 0L
        val path = "/v1/library/assets/" + Api.part(item.assetId) + "/download" + if (item.virtualJob.isEmpty()) "" else "?virtual=" + item.virtualJob
        try {
            val connection = URL(api.url(path)).openConnection() as HttpURLConnection
            connection.connectTimeout = 15000; connection.readTimeout = 60000
            connection.setRequestProperty("Authorization", "Bearer " + api.token)
            connection.setRequestProperty("Accept-Encoding", "identity")
            if (have > 0) connection.setRequestProperty("Range", "bytes=$have-")
            val code = connection.responseCode
            if (code == 401 || code == 403) throw Api.AuthException(L10n.t("The connection has expired or was revoked. Link this phone again."))
            if (code >= 400) throw Api.HttpException(code, Api.read(connection.errorStream ?: connection.inputStream, 65536).take(300))
            // A server that sends the whole file again: start over.
            if (code == 200) have = 0
            val manager = getSystemService(NotificationManager::class.java)
            var lastShown = 0L; var lastSaved = have
            connection.inputStream.use { input ->
                FileOutputStream(partial, have > 0).use { output ->
                    val buffer = ByteArray(256 * 1024)
                    while (true) {
                        val state = Saved.item(item.id)?.state
                        if (!running || state == null || state == "paused") { connection.disconnect(); break }
                        val n = input.read(buffer)
                        if (n < 0) break
                        output.write(buffer, 0, n)
                        have += n
                        val now = System.currentTimeMillis()
                        if (now - lastShown > 500) {
                            lastShown = now
                            val received = have
                            main.post { Saved.live = item.id to received }
                            val percent = if (item.size > 0) (received * 100 / item.size).toInt() else 0
                            manager.notify(NOTICE, notice(item.title + (if (item.hasSeason) " · " + item.label else ""), percent))
                        }
                        if (have - lastSaved > 32L shl 20) { lastSaved = have; val received = have; main.post { Saved.update(item.id) { it.copy(received = received) } } }
                    }
                }
            }
            val received = have
            if (item.size in 1..received) {
                partial.renameTo(Saved.media(item))
                main.post { Saved.update(item.id) { it.copy(received = received, state = "ready") } }
            } else main.post {
                Saved.update(item.id) { it.copy(received = received, state = if (it.state == "downloading") (if (running) "waiting" else "queued") else it.state) }
            }
        } catch (e: Api.AuthException) {
            val received = have
            main.post { Saved.update(item.id) { it.copy(received = received, state = "failed", error = e.message ?: "") } }
        } catch (e: Api.HttpException) {
            val received = have
            main.post { Saved.update(item.id) { it.copy(received = received, state = "failed", error = e.message ?: "") } }
        } catch (e: IOException) {
            // The network went away: waits for the next one (Saved.kick).
            val received = have
            main.post { Saved.update(item.id) { it.copy(received = received, state = "waiting") } }
            running = false
        } catch (e: Exception) {
            val received = have
            main.post { Saved.update(item.id) { it.copy(received = received, state = "failed", error = MainActivity.message(e)) } }
        }
    }

    companion object {
        private const val CHANNEL = "downloads"
        private const val NOTICE = 7
    }
}
