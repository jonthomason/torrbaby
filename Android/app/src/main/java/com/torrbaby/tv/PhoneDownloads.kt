package com.torrbaby.tv

import android.os.Handler
import android.os.Looper
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay
import org.json.JSONObject
import java.util.Locale
import java.util.concurrent.Executors

/** The server's downloads (Transmission's torrents) and what the library matched each to. */
object Torrents {
    private val io = Executors.newSingleThreadExecutor()
    private val main = Handler(Looper.getMainLooper())
    var all by mutableStateOf<List<JSONObject>>(emptyList())
        private set
    /** The library's title for each download, by hash: TMDB's name and year. */
    var titles by mutableStateOf<Map<String, JSONObject>>(emptyMap())
        private set
    var loaded by mutableStateOf(false)
        private set
    /** Needs attention entries already seen, so the menu marks only new ones (as on the iPhone). */
    var seenAttention by mutableStateOf<Set<String>>(emptySet())

    fun refresh() {
        val api = Phone.api ?: return
        io.execute {
            try {
                val list = api.array("/v1/torrents").let { a -> (0 until a.length()).map { a.getJSONObject(it) } }
                val matched = try {
                    api.array("/v1/metadata").let { a -> (0 until a.length()).map { a.getJSONObject(it) } }
                        .mapNotNull { m -> m.optJSONObject("selected")?.takeIf { it.optString("title").isNotEmpty() }?.let { m.optString("hash") to it } }.toMap()
                } catch (_: Exception) { titles }
                main.post { all = list; titles = matched; loaded = true }
            } catch (e: Exception) { main.post { Phone.problem = MainActivity.message(e) } }
        }
    }

    /** A request on one download, then the list again; failures go to the banner. */
    fun act(hash: String, action: String, body: JSONObject = JSONObject(), done: () -> Unit = {}) {
        val api = Phone.api ?: return
        io.execute {
            try { api.request("POST", "/v1/torrents/" + Api.part(hash) + "/" + action, body); main.post(done) }
            catch (e: Exception) { main.post { Phone.problem = MainActivity.message(e) } }
            refresh()
        }
    }

    fun add(item: JSONObject, paused: Boolean, done: (String?) -> Unit) {
        val api = Phone.api ?: return
        io.execute {
            try {
                val magnet = item.optString("magnet")
                if (magnet.isEmpty()) throw Exception(L10n.t("This release has no downloadable torrent."))
                val response = api.request("POST", "/v1/torrents", JSONObject().put("paused", paused).put("magnet", magnet))
                main.post { done(L10n.t(if (response.has("torrent_duplicate")) "Already in your torrents" else "Torrent added")) }
                refresh()
            } catch (e: Exception) { main.post { done(null); Phone.problem = MainActivity.message(e) } }
        }
    }

    val attention: List<JSONObject> get() = all.filter { issue(it) != null && it.optJSONObject("file_operation")?.optString("action") != "junk" }
    val hasNewAttention: Boolean get() = attention.any { it.optString("hash_string") !in seenAttention }
    fun markAttentionSeen() { seenAttention = attention.map { it.optString("hash_string") }.toSet() }
}

enum class Shelf(val label: String) { Downloading("Downloading"), Seeding("Seeding"), Complete("Complete"), Attention("Needs attention") }

private fun JSONObject.done() = optDouble("percent_done") >= 1.0
private fun JSONObject.localError() = optInt("error") == 3

/** Why a download needs attention, as on the iPhone (Models.swift progressIssue), or null. */
fun issue(t: JSONObject): String? {
    if (t.done()) return if (t.localError()) t.optString("error_string") else t.optString("library_issue").takeIf { it.isNotEmpty() && !t.isNull("library_issue") }?.let { L10n.t(it) }
    if (t.localError()) return t.optString("error_string")
    val status = t.optInt("status")
    if (status == 0) return null
    if (t.optBoolean("is_stalled")) return L10n.t("Transmission reports this torrent as stalled")
    if (status == 4 && t.optLong("rate_download") == 0L && System.currentTimeMillis() / 1000.0 - maxOf(t.optDouble("added_date"), t.optDouble("activity_date")) > 300)
        return L10n.t(if (t.optInt("peers_connected") == 0) "No peers and no activity for over 5 minutes" else "Peers connected, but no activity for over 5 minutes")
    return null
}

private fun inShelf(t: JSONObject, shelf: Shelf): Boolean {
    val moving = t.optJSONObject("file_operation") != null
    return when (shelf) {
        Shelf.Downloading -> !moving && !t.done()
        Shelf.Seeding -> moving || (t.done() && t.optInt("status") == 6)
        Shelf.Complete -> !moving && t.done() && t.optInt("status") != 6
        Shelf.Attention -> t.optJSONObject("file_operation")?.optString("action") != "junk" && issue(t) != null
    }
}

private fun stateName(t: JSONObject): String {
    if (t.localError()) return L10n.t("Needs attention")
    return L10n.t(when (t.optInt("status")) {
        0 -> "Paused"; 1 -> "Waiting to verify"; 2 -> "Verifying"; 3 -> "Queued"
        4 -> if (t.optBoolean("is_stalled")) "Waiting for peers" else "Downloading"
        5 -> "Queued to seed"; 6 -> "Seeding"; else -> "Status unavailable"
    })
}

/** Megabits per second, as the iPhone shows speeds. */
private fun mbps(bytesPerSecond: Long): String {
    val value = bytesPerSecond * 8 / 1_000_000.0
    return if (value <= 0) "0 Mbps" else if (value < 10) String.format(Locale.ROOT, "%.1f Mbps", value) else String.format(Locale.ROOT, "%.0f Mbps", value)
}

private fun eta(t: JSONObject): String? {
    if (t.optInt("status") != 4 || t.optBoolean("is_stalled")) return null
    val seconds = t.optLong("smoothed_eta", t.optLong("eta", -1))
    if (seconds < 0) return L10n.t("Estimating…")
    return when {
        seconds < 60 -> L10n.t("<1m remaining")
        seconds < 3600 -> String.format(L10n.locale(), L10n.t("~%dm remaining"), maxOf(1, seconds / 60))
        seconds < 86400 -> String.format(L10n.locale(), L10n.t("~%dh %dm remaining"), seconds / 3600, (seconds % 3600) / 60)
        else -> String.format(L10n.locale(), L10n.t("~%dd %dh remaining"), seconds / 86400, (seconds % 86400) / 3600)
    }
}

private fun size(t: JSONObject): Long = t.optJSONArray("files")?.let { f -> (0 until f.length()).sumOf { f.getJSONObject(it).optLong("length") } } ?: 0

/** The library's title and year, else the release's name up to its year, season or first tag. */
private fun titleOf(t: JSONObject): Pair<String, String> {
    Torrents.titles[t.optString("hash_string")]?.let { return it.optString("title") to it.optString("year") }
    val name = t.optString("name").replace('.', ' ').replace('_', ' ').replace('+', ' ')
    val cut = Regex("(?i)[\\[(【]|\\b(19|20)\\d{2}\\b|\\bS\\d{1,2}(E\\d{1,3})?\\b|\\b(2160p|1080p|720p|480p|4k|bluray|web-?dl|webrip|hdtv|x26[45]|h ?26[45]|hevc)\\b").find(name)
    val head = (cut?.let { name.substring(0, it.range.first) } ?: name).trim(' ', '-', '–', '—', '.')
    val year = t.optJSONObject("hints")?.optInt("year")?.takeIf { it > 0 }?.toString() ?: ""
    return (if (head.length >= 2) head else name) to year
}

/** The release's tags as Find shows them (season and episodes, picture, sound, source); the year is in the title. */
private fun releaseTags(t: JSONObject): String =
    TorrentSearchActivity.tags(JSONObject().put("hints", JSONObject((t.optJSONObject("hints") ?: JSONObject()).toString()).apply { remove("year") }))

/** Whole percent, rounded down: 99.9% is not shown as done. */
private fun percent(fraction: Double) = String.format(Locale.ROOT, "%d%%", (fraction * 100).toInt().coerceIn(0, 100))

@Composable
fun DownloadsPage(shelf: Shelf, open: (String) -> Unit) {
    LaunchedEffect(shelf) { while (true) { Torrents.refresh(); delay(5000) } }
    if (shelf == Shelf.Attention) LaunchedEffect(Torrents.all) { Torrents.markAttentionSeen() }
    val list = Torrents.all.filter { inShelf(it, shelf) }.sortedByDescending { it.optDouble("added_date") }
    if (!Torrents.loaded) return Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { Words(L10n.t("Loading…"), color = Ink.muted) }
    if (list.isEmpty()) return Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { Words(L10n.t("Nothing here"), color = Ink.muted) }
    val down = Torrents.all.sumOf { it.optLong("rate_download") }; val up = Torrents.all.sumOf { it.optLong("rate_upload") }
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(start = 12.dp, end = 12.dp, bottom = bottomRoom())) {
        item { Words("↓ " + mbps(down) + "   ↑ " + mbps(up), 13.sp, FontWeight.SemiBold, Ink.muted, modifier = Modifier.padding(4.dp, 4.dp, 4.dp, 10.dp)) }
        items(list, key = { it.optString("hash_string") }) { TransferRow(it, shelf == Shelf.Attention, open) }
    }
}

@Composable
private fun TransferRow(t: JSONObject, showIssue: Boolean, open: (String) -> Unit) {
    val hash = t.optString("hash_string")
    val (title, year) = titleOf(t)
    val done = t.optDouble("percent_done")
    val context = androidx.compose.ui.platform.LocalContext.current
    Row(Modifier.fillMaxWidth().padding(vertical = 6.dp).clip(RoundedCornerShape(12.dp)).background(Ink.panel).clickable { open(hash) }.padding(10.dp)) {
        Artwork("torrent:$hash", "poster", hash, Modifier.width(54.dp).aspectRatio(2f / 3f).clip(RoundedCornerShape(6.dp)))
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Words(if (year.isEmpty()) title else "$title ($year)", 15.sp, FontWeight.SemiBold, lines = 2)
            releaseTags(t).takeIf { it.isNotEmpty() }?.let { Words(it, 12.sp, color = Ink.muted, lines = 1) }
            Spacer(Modifier.height(6.dp))
            // Two lines: what it is doing and how far; then how fast and how long, or how it seeds.
            val first = listOf(stateName(t), String.format(L10n.locale(), L10n.t("%s of %s"), percent(done), android.text.format.Formatter.formatShortFileSize(context, size(t))),
                String.format(L10n.locale(), L10n.t("%d peers"), t.optInt("peers_connected"))).joinToString(" · ")
            Words(first, 12.sp, color = Ink.text, lines = 1)
            val second = if (t.done()) listOf("↑ " + mbps(t.optLong("rate_upload")), String.format(L10n.locale(), L10n.t("Ratio %s"), String.format(Locale.ROOT, "%.2f", maxOf(0.0, t.optDouble("upload_ratio")))))
                else listOfNotNull("↓ " + mbps(t.optLong("rate_download")), eta(t))
            Words(second.joinToString(" · "), 12.sp, color = Ink.muted, lines = 1)
            if (showIssue) issue(t)?.let { Words(it, 12.sp, color = Color(0xFFFF9F0A), lines = 2) }
            Spacer(Modifier.height(6.dp))
            Box(Modifier.fillMaxWidth().height(4.dp).clip(RoundedCornerShape(2.dp)).background(Ink.control)) {
                Box(Modifier.fillMaxWidth(done.toFloat().coerceIn(0f, 1f)).fillMaxHeight().background(if (t.done()) Ink.green else Ink.text))
            }
        }
    }
}

@Composable
fun TransferDetail(hash: String, close: () -> Unit) {
    LaunchedEffect(hash) { while (true) { Torrents.refresh(); delay(5000) } }
    val context = androidx.compose.ui.platform.LocalContext.current
    val t = Torrents.all.firstOrNull { it.optString("hash_string") == hash } ?: return Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { Words(L10n.t("Loading…"), color = Ink.muted) }
    var removing by remember { mutableStateOf(false) }
    var more by remember { mutableStateOf(false) }
    val (title, year) = titleOf(t)
    val paused = t.optInt("status") == 0
    val first = !t.done() && t.optInt("bandwidth_priority") > 0
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(start = 16.dp, end = 16.dp, bottom = bottomRoom())) {
        item {
            Row {
                Artwork("torrent:$hash", "poster", hash, Modifier.width(92.dp).aspectRatio(2f / 3f).clip(RoundedCornerShape(8.dp)))
                Spacer(Modifier.width(14.dp))
                Column(Modifier.weight(1f)) {
                    Words(title, 20.sp, FontWeight.Bold, lines = 3)
                    if (year.isNotEmpty()) Words(year, 14.sp, color = Ink.muted)
                    releaseTags(t).takeIf { it.isNotEmpty() }?.let { Spacer(Modifier.height(4.dp)); Words(it, 12.sp, color = Ink.muted) }
                }
            }
            Spacer(Modifier.height(10.dp))
            Words(t.optString("name"), 12.sp, color = Ink.muted)
            Spacer(Modifier.height(14.dp))
            Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(Ink.panel).padding(14.dp)) {
                Words(stateName(t) + " · " + percent(t.optDouble("percent_done")), 15.sp, FontWeight.SemiBold)
                Spacer(Modifier.height(6.dp))
                Words(listOfNotNull("↓ " + mbps(t.optLong("rate_download")), "↑ " + mbps(t.optLong("rate_upload")), eta(t),
                    String.format(L10n.locale(), L10n.t("%d peers"), t.optInt("peers_connected"))).joinToString(" · "), 13.sp, color = Ink.muted)
                issue(t)?.let { Spacer(Modifier.height(6.dp)); Words(it, 13.sp, color = Color(0xFFFF9F0A)) }
            }
            Spacer(Modifier.height(14.dp))
            Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                if (t.optJSONObject("file_operation") == null) Action(L10n.t(if (paused) "Resume" else "Pause"), prominent = true) { Torrents.act(hash, if (paused) "start" else "stop") }
                if (!t.done()) Action(L10n.t(if (first) "Stop downloading first" else "Download first")) { Torrents.act(hash, if (first) "unprioritize" else "prioritize") }
                Action(L10n.t("More")) { more = true }
                Action(L10n.t("Remove")) { removing = true }
            }
            Spacer(Modifier.height(18.dp))
            Words(L10n.t("Files"), 15.sp, FontWeight.Bold)
            Spacer(Modifier.height(6.dp))
        }
        val files = t.optJSONArray("files")?.let { f -> (0 until f.length()).map { f.getJSONObject(it) } } ?: emptyList()
        items(files.size) { index ->
            val f = files[index]
            val length = f.optLong("length")
            Column(Modifier.fillMaxWidth().padding(vertical = 6.dp)) {
                Words(f.optString("name").substringAfterLast('/'), 13.sp, lines = 2)
                Words(String.format(Locale.ROOT, "%.0f%%", if (length > 0) f.optLong("bytes_completed") * 100.0 / length else 0.0) + " · " +
                    android.text.format.Formatter.formatShortFileSize(context, length), 11.sp, color = Ink.muted)
            }
        }
    }
    if (more) {
        val actions = listOf("Verify files" to "verify", "Ask trackers for peers" to "reannounce", "Retry organization" to "retry", "Ignore organization" to "ignore")
        Choice(actions.map { L10n.t(it.first) }, -1, { more = false }) { index -> more = false; Torrents.act(hash, actions[index].second) }
    }
    if (removing) RemoveAlert(if (year.isEmpty()) title else "$title ($year)", { removing = false }) { delete ->
        removing = false
        Torrents.act(hash, "remove", JSONObject().put("delete_data", delete).put("confirm_hash", hash)) { close() }
    }
}

/** Remove “Title”? with Keep files, Delete files and Cancel, as on the iPhone. */
@Composable
private fun RemoveAlert(title: String, cancel: () -> Unit, remove: (Boolean) -> Unit) {
    BackHandler(onBack = cancel)
    Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.6f)).clickable(onClick = cancel), contentAlignment = Alignment.Center) {
        Column(Modifier.fillMaxWidth(0.82f).clip(RoundedCornerShape(16.dp)).background(Ink.panel).clickable(enabled = false) {}.padding(18.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Words(String.format(L10n.locale(), L10n.t("Remove “%s”?"), title), 17.sp, FontWeight.Bold)
            Spacer(Modifier.height(4.dp))
            Action(L10n.t("Keep files"), modifier = Modifier.fillMaxWidth()) { remove(false) }
            Box(Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).background(Color(0xFF5A1D1D)).clickable { remove(true) }.padding(vertical = 11.dp), contentAlignment = Alignment.Center) {
                Words(L10n.t("Delete files"), 15.sp, FontWeight.SemiBold, Color(0xFFFF6B6B))
            }
            Action(L10n.t("Cancel"), modifier = Modifier.fillMaxWidth()) { cancel() }
        }
    }
}
