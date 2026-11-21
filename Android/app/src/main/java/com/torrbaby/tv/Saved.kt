package com.torrbaby.tv

import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.net.ConnectivityManager
import android.net.Network
import android.os.Handler
import android.os.Looper
import android.os.StatFs
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.util.UUID
import java.util.concurrent.Executors

/** One title or episode kept on this phone (iOS DownloadStorage.Item). */
data class SavedItem(
    val id: String,
    /** The link it was fetched over (SecureStore's client): progress goes back only through it. */
    val client: String,
    val assetId: String,
    val titleId: String,
    val title: String,
    val label: String,
    val heading: String,
    val season: Int,
    val episode: Int,
    val hasSeason: Boolean,
    val size: Long = 0,
    val received: Long = 0,
    val etag: String = "",
    val virtualJob: String = "",
    val extension: String = "mp4",
    /** preparing, queued, downloading, waiting (for a network), paused, ready, failed. */
    val state: String = "preparing",
    val error: String = "",
    val position: Double = 0.0,
    val duration: Double = 0.0,
    val watched: Boolean = false,
    val recordedAt: Double = 0.0,
    val profile: String = "",
    /** A playback report not yet acknowledged by Diamond (POST /v1/playback/offline). */
    val pendingEvent: String = "",
    val reachedEnd: Boolean = false,
    val credits: Long = -1,
) {
    val active get() = state == "preparing" || state == "queued" || state == "downloading"
    val fraction get() = if (size > 0) (received.toDouble() / size).coerceIn(0.0, 1.0) else 0.0

    fun json(): JSONObject = JSONObject().put("id", id).put("client", client).put("asset_id", assetId).put("title_id", titleId)
        .put("title", title).put("label", label).put("heading", heading).put("season", season).put("episode", episode)
        .put("has_season", hasSeason).put("size", size).put("received", received).put("etag", etag).put("virtual_job", virtualJob)
        .put("extension", extension).put("state", state).put("error", error).put("position", position).put("duration", duration)
        .put("watched", watched).put("recorded_at", recordedAt).put("profile", profile).put("pending_event", pendingEvent)
        .put("reached_end", reachedEnd).put("credits", credits)

    companion object {
        fun from(j: JSONObject) = SavedItem(
            j.getString("id"), j.optString("client"), j.getString("asset_id"), j.optString("title_id"), j.optString("title"),
            j.optString("label"), j.optString("heading"), j.optInt("season"), j.optInt("episode"), j.optBoolean("has_season"),
            j.optLong("size"), j.optLong("received"), j.optString("etag"), j.optString("virtual_job"), j.optString("extension", "mp4"),
            j.optString("state"), j.optString("error"), j.optDouble("position", 0.0), j.optDouble("duration", 0.0),
            j.optBoolean("watched"), j.optDouble("recorded_at", 0.0), j.optString("profile"), j.optString("pending_event"),
            j.optBoolean("reached_end"), j.optLong("credits", -1))
    }
}

/**
 * Titles saved to the phone for watching without the server (the iPhone's Downloads). Diamond
 * packs a file's ready subtitles into the copy (POST virtual-mp4, as iOS does), so it plays with
 * them offline. Transfers run in SavedService and continue from where they stopped; watching a
 * copy is reported to Diamond when it can next be reached.
 */
object Saved {
    private val main = Handler(Looper.getMainLooper())
    /** Preparing copies (one at a time, so Diamond is not asked to build many at once). */
    private val preparing = Executors.newSingleThreadExecutor()
    private val writes = Executors.newSingleThreadExecutor()
    private lateinit var app: Context
    private lateinit var root: File

    val items = mutableStateListOf<SavedItem>()
    /** Bytes received by the running transfer, updated often (the list is saved less often). */
    var live by mutableStateOf<Pair<String, Long>?>(null)
    var waitingForWiFi by mutableStateOf(false)
    var error by mutableStateOf<String?>(null)

    fun attach(context: Context) {
        if (::app.isInitialized) return
        app = context.applicationContext
        root = File(app.filesDir, "downloads").apply { mkdirs() }
        try {
            val list = JSONArray(File(root, "index.json").readText())
            for (i in 0 until list.length()) items.add(SavedItem.from(list.getJSONObject(i)))
        } catch (_: Exception) { }
        // A transfer cut off when the app was ended starts again where it stopped.
        for (item in items.toList()) if (item.state == "downloading") update(item.id) { it.copy(state = "queued") }
        (app.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager).registerNetworkCallback(realNetworks, object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) { main.post { kick() } }
        })
    }

    /** Wi-Fi, mobile or wired networks, not a VPN (ABT's stays up without any of them). */
    val realNetworks: android.net.NetworkRequest = android.net.NetworkRequest.Builder()
        .addCapability(android.net.NetworkCapabilities.NET_CAPABILITY_INTERNET)
        .addCapability(android.net.NetworkCapabilities.NET_CAPABILITY_NOT_VPN).build()

    fun media(item: SavedItem) = File(root, item.id + "." + item.extension)
    fun partial(item: SavedItem) = File(root, item.id + ".part")
    fun poster(item: SavedItem) = File(root, item.id + ".jpg")
    fun item(id: String) = items.firstOrNull { it.id == id }
    fun forAsset(assetId: String) = items.firstOrNull { it.assetId == assetId }
    fun ready(assetId: String) = items.firstOrNull { it.assetId == assetId && it.state == "ready" && media(it).exists() }

    val used: Long get() = items.sumOf { if (it.state == "ready") media(it).length() else partial(it).length() }
    val free: Long get() = try { StatFs(root.path).availableBytes } catch (_: Exception) { 0 }

    /** Change one item and save the list (main thread). */
    fun update(id: String, change: (SavedItem) -> SavedItem) {
        val index = items.indexOfFirst { it.id == id }
        if (index < 0) return
        items[index] = change(items[index])
        save()
    }

    private fun save() {
        val snapshot = JSONArray().also { list -> items.forEach { list.put(it.json()) } }.toString()
        writes.execute {
            val temporary = File(root, "index.json.tmp")
            temporary.writeText(snapshot)
            temporary.renameTo(File(root, "index.json"))
        }
    }

    /** Save a title or episode: Diamond prepares the copy, then it joins the queue. */
    fun add(title: Library.Title, asset: Library.Asset) {
        val api = Phone.api ?: return
        if (items.any { it.assetId == asset.id && it.state != "failed" }) return
        items.firstOrNull { it.assetId == asset.id }?.let { remove(it.id) }
        val item = SavedItem(UUID.randomUUID().toString(), api.client, asset.id, title.id, title.title, asset.label(), asset.heading(title),
            asset.season, asset.episode, asset.hasSeason, position = asset.position, duration = asset.duration, watched = asset.watched,
            recordedAt = asset.lastPlayed, profile = api.profile)
        items.add(item); save()
        Phone.art?.bitmap(title.id, "poster", title.poster) { picture -> keepPoster(item, picture) }
        preparing.execute {
            try {
                val prepared = prepare(api, asset.id)
                main.post { update(item.id) { it.copy(size = prepared.size, etag = prepared.etag, virtualJob = prepared.virtualJob, extension = prepared.extension, credits = prepared.credits, state = "queued") }; kick() }
            } catch (e: Exception) {
                main.post { update(item.id) { it.copy(state = "failed", error = MainActivity.message(e)) } }
            }
        }
    }

    private class Prepared(val size: Long, val etag: String, val virtualJob: String, val extension: String, val credits: Long)

    /** As iOS's mediaPlaybackLink: every ready subtitle packed into one MP4 with the video. */
    private fun prepare(api: Api, asset: String): Prepared {
        val base = "/v1/library/assets/" + Api.part(asset)
        val ids = JSONArray(); val names = JSONArray()
        api.request("GET", "$base/media-info", null).optJSONArray("prepared")?.let { jobs ->
            val seen = HashSet<String>()
            for (i in 0 until jobs.length()) {
                val job = jobs.getJSONObject(i)
                if (job.optString("state") != "ready" || !seen.add(job.optString("id"))) continue
                ids.put(job.optString("id"))
                names.put(MediaLanguages.trackName(job.optString("language"), job.optString("kind", "text"), job.optBoolean("sdh")))
            }
        }
        var virtualJob = ""
        if (ids.length() > 0) {
            var job = api.request("POST", "$base/virtual-mp4", JSONObject().put("subtitle_jobs", ids).put("subtitle_names", names))
            if (job.optString("state") != "direct") {
                val deadline = System.currentTimeMillis() + 20 * 60_000
                while (job.optString("state") in setOf("queued", "running", "preparing")) {
                    if (System.currentTimeMillis() > deadline) throw Exception(L10n.t("Download failed"))
                    Thread.sleep(1000)
                    job = api.request("GET", "/v1/virtual-jobs/" + Api.part(job.getString("id")), null)
                }
                if (job.optString("state") != "ready") throw Exception(job.optString("error").ifEmpty { L10n.t("Download failed") })
                virtualJob = job.getString("id")
            }
        }
        val info = api.request("POST", "$base/download-info", if (virtualJob.isEmpty()) JSONObject() else JSONObject().put("virtual_job", virtualJob))
        val credits = try { EpisodeAdvance.closingCredits(api.request("GET", "$base/credits", null)) } catch (_: Exception) { -1L }
        val extension = info.optString("name").substringAfterLast('.', "mp4").lowercase().takeIf { it.length in 2..4 } ?: "mp4"
        return Prepared(info.getLong("size"), info.optString("etag"), virtualJob, extension, credits)
    }

    private fun keepPoster(item: SavedItem, picture: Bitmap?) {
        picture ?: return
        writes.execute { try { poster(item).outputStream().use { picture.compress(Bitmap.CompressFormat.JPEG, 85, it) } } catch (_: Exception) { } }
    }

    fun pause(id: String) = update(id) { if (it.state == "ready") it else it.copy(state = "paused") }
    fun resume(id: String) { update(id) { it.copy(state = if (it.size > 0) "queued" else "failed", error = "") }; kick() }

    fun remove(id: String) {
        val item = item(id) ?: return
        items.removeAll { it.id == id }; save()
        writes.execute { media(item).delete(); partial(item).delete(); poster(item).delete() }
    }

    // The network ------------------------------------------------------------------------------

    /** Cellular and hotspot use is allowed until then (three hours from asking, as on the iPhone). */
    private var meteredUntil: Long
        get() = app.getSharedPreferences("downloads", Context.MODE_PRIVATE).getLong("metered_until", 0)
        set(value) { app.getSharedPreferences("downloads", Context.MODE_PRIVATE).edit().putLong("metered_until", value).apply() }

    fun allowMeteredForAWhile() { meteredUntil = System.currentTimeMillis() + 3 * 3600_000L; kick() }

    fun mayUseNetwork(): Boolean {
        val metered = (app.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager).isActiveNetworkMetered
        val allowed = !metered || System.currentTimeMillis() < meteredUntil
        main.post { waitingForWiFi = !allowed && items.any { it.state == "queued" || it.state == "waiting" } }
        return allowed
    }

    /** Start the transfers if any are due (opening the app, a network appearing, a new download). */
    fun kick() {
        if (!::app.isInitialized) return
        for (item in items.toList()) if (item.state == "waiting") update(item.id) { it.copy(state = "queued") }
        if (items.none { it.state == "queued" } || !mayUseNetwork()) return
        try { app.startForegroundService(Intent(app, SavedService::class.java)) }
        catch (_: Exception) { /* In the background Android may refuse; the app's next opening starts them. */ }
    }

    // Watching ---------------------------------------------------------------------------------

    /** Where a copy was left, kept here and reported to Diamond when it can be reached. */
    fun progress(id: String, profile: String, position: Double, duration: Double, watched: Boolean, reachedEnd: Boolean) = main.post {
        update(id) { it.copy(position = position, duration = duration, watched = it.watched || watched, reachedEnd = it.reachedEnd || reachedEnd,
            recordedAt = System.currentTimeMillis() / 1000.0, profile = profile, pendingEvent = UUID.randomUUID().toString()) }
        sync()
    }

    /** Report watching done on copies (POST /v1/playback/offline), each once acknowledged. */
    fun sync() {
        val api = Phone.api ?: return
        val due = items.filter { it.pendingEvent.isNotEmpty() && it.client == api.client && it.profile.isNotEmpty() }
        if (due.isEmpty()) return
        writes.execute {
            for (item in due) try {
                val reply = api.request("POST", "/v1/playback/offline", JSONObject().put("event_id", item.pendingEvent).put("client_id", api.client)
                    .put("asset_id", item.assetId).put("profile_id", item.profile).put("position", item.position).put("duration", item.duration)
                    .put("recorded_at", item.recordedAt).put("reached_end", item.reachedEnd).put("watched_eligible", item.watched))
                if (reply.optBoolean("acknowledged")) main.post { update(item.id) { if (it.pendingEvent == item.pendingEvent) it.copy(pendingEvent = "") else it } }
            } catch (_: Exception) { /* Kept for the next time Diamond answers. */ }
        }
    }
}
