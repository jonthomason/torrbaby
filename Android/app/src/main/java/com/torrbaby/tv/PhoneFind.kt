package com.torrbaby.tv

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.os.Handler
import android.os.Looper
import android.util.Base64
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.json.JSONArray
import org.json.JSONObject
import java.util.concurrent.Executors

/**
 * Find, as on the iPhone: a title by name (TMDB), then its releases (whole seasons or one season's
 * episodes for a series), or releases by the words typed; each opens with its tags, files and Add.
 * Kept here, not in the page, so the results are still there coming back from a release.
 */
object Find {
    private val io = Executors.newSingleThreadExecutor()
    private val main = Handler(Looper.getMainLooper())
    private var generation = 0
    var query by mutableStateOf("")
    var kind by mutableStateOf("movie")
    var china by mutableStateOf(false)
    var busy by mutableStateOf(false)
    var note by mutableStateOf("")
    var titles by mutableStateOf<List<JSONObject>>(emptyList())
    var chosen by mutableStateOf<JSONObject?>(null)
    var seasons by mutableStateOf<List<Int>>(emptyList())
    var episodes by mutableStateOf(false)
    var season by mutableIntStateOf(0)
    var results by mutableStateOf<List<JSONObject>?>(null)
    var bySize by mutableStateOf(false)
    var searched = ""
    /** ✕ in the box: the words and everything found for them go, ready for the next search. */
    fun clear() {
        generation++
        busy = false; note = ""; query = ""; titles = emptyList(); chosen = null; seasons = emptyList(); results = null; searched = ""
    }

    private fun start(label: String): Int { busy = true; note = L10n.t(label); return ++generation }
    private fun failed(request: Int, e: Exception) = main.post { if (request == generation) { busy = false; note = MainActivity.message(e) } }

    fun findTitles() {
        val value = query.trim(); val api = Phone.api ?: return
        if (value.length < 2) return
        val request = start("Searching…"); chosen = null; results = null; titles = emptyList()
        val wanted = kind
        io.execute {
            try {
                val found = api.request("POST", "/v1/search/titles", JSONObject().put("query", value).put("kind", wanted)).optJSONArray("titles").list()
                main.post { if (request == generation) { busy = false; titles = found; note = if (found.isEmpty()) L10n.t("No titles found") else "" } }
            } catch (e: Exception) { failed(request, e) }
        }
    }

    fun choose(title: JSONObject) {
        chosen = title; titles = emptyList(); episodes = false; season = 0; seasons = emptyList()
        if (title.optString("kind") != "tv") { searchTitle(); return }
        val api = Phone.api ?: return
        val request = start("Searching…")
        io.execute {
            val aired = try {
                api.request("POST", "/v1/search/titles/facts", JSONObject().put("id", title.optString("id"))).optJSONArray("seasons").list()
                    .filter { it.optInt("aired") > 0 }.map { it.optInt("season") }
            } catch (_: Exception) { emptyList() }
            main.post { if (request == generation) { seasons = aired; searchTitle() } }
        }
    }

    fun searchTitle() {
        val title = chosen ?: return; val api = Phone.api ?: return
        val request = start(if (china) "Searching and checking links…" else "Searching…")
        val body = JSONObject().put("title", title.optString("id")).put("market", if (china) "china" else "world")
            .put("want", if (episodes) "episodes" else "season").put("sources", JSONArray())
        if (season > 0) body.put("season", season)
        io.execute { try { show(request, api.request("POST", "/v1/search/torrents", body)) } catch (e: Exception) { failed(request, e) } }
    }

    fun searchText() {
        val value = query.trim(); val api = Phone.api ?: return
        if (value.length < 2) return
        chosen = null; titles = emptyList()
        val request = start(if (china) "Searching and checking links…" else "Searching…")
        val body = JSONObject().put("query", value).put("market", if (china) "china" else "world").put("sources", JSONArray())
        io.execute { try { show(request, api.request("POST", "/v1/search/torrents", body)) } catch (e: Exception) { failed(request, e) } }
    }

    private fun show(request: Int, response: JSONObject) {
        val items = response.optJSONArray("items").list()
        val problems = response.optJSONArray("providers").list().filter { !it.isNull("error") }
            .joinToString("\n") { TorrentSearchActivity.label(it.optString("provider")) + ": " + it.optString("error") }
        main.post {
            if (request != generation) return@post
            busy = false; searched = response.optString("query", query); results = items
            note = listOf(if (items.isEmpty()) L10n.t("No results") else "", problems).filter { it.isNotEmpty() }.joinToString("\n")
        }
    }

    fun ordered(): List<JSONObject> = results?.let { list ->
        if (bySize) list.sortedByDescending { it.optLong("size_bytes") } else list.sortedByDescending { it.optLong("seeders") }
    } ?: emptyList()

    fun addCloud(item: JSONObject, saveAs: String, done: (String?) -> Unit) {
        val api = Phone.api ?: return
        io.execute {
            try {
                val copy = item.optJSONObject("cloud")
                val body = JSONObject().put("url", TorrentSearchActivity.shareUrl(item)).put("title", saveAs)
                if (copy != null && !copy.isNull("key")) { body.put("copy", copy.optString("key")); if (copy.optInt("season") > 0) body.put("season", copy.optInt("season")) }
                else if (TorrentSearchActivity.seasonIn(item.optString("name")) > 0) body.put("season", TorrentSearchActivity.seasonIn(item.optString("name")))
                val result = api.request("POST", "/v1/cloud/add", body)
                val had = result.optInt("already_have"); val all = result.optInt("total")
                val message = when {
                    result.optBoolean("already_queued") -> L10n.t("Already in the download queue.")
                    had > 0 && had >= all -> String.format(L10n.locale(), L10n.t("Already in your library: all %d videos. Nothing to download."), all)
                    had > 0 -> String.format(L10n.locale(), L10n.t("%d of %d videos are already in your library; the rest will download."), had, all)
                    else -> L10n.t("Follow its progress in Downloading.")
                }
                main.post { done(message) }
            } catch (e: Exception) { main.post { done(null); Phone.problem = MainActivity.message(e) } }
        }
    }

    /** A release's description and files (and, from BT之家, its attachments), fetched when it is opened. */
    fun detail(item: JSONObject, done: (JSONObject?) -> Unit) {
        val api = Phone.api ?: return done(null)
        val sources = item.optJSONArray("sources").list()
        val source = sources.firstOrNull { it.optString("provider") == "tpb" } ?: sources.firstOrNull { it.optString("provider") == "nyaa" } ?: sources.firstOrNull() ?: return done(null)
        io.execute {
            val response = try { api.request("POST", "/v1/search/torrents/detail", JSONObject().put("provider", source.optString("provider")).put("id", source.optString("id"))) } catch (_: Exception) { null }
            main.post { done(response) }
        }
    }
}

fun JSONArray?.list(): List<JSONObject> = if (this == null) emptyList() else (0 until length()).mapNotNull { optJSONObject(it) }

@Composable
private fun Chip(label: String, selected: Boolean, onClick: () -> Unit) = Action(label, prominent = selected, onClick = onClick)

@Composable
fun FindPage(openRelease: (JSONObject) -> Unit) {
    val focus = LocalFocusManager.current
    val search = { focus.clearFocus(); Find.findTitles() }
    LazyColumn(Modifier.fillMaxSize().dismissesKeyboard(), contentPadding = PaddingValues(start = 12.dp, end = 12.dp, bottom = bottomRoom())) {
        item {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                SearchField(Find.query, { Find.query = it }, L10n.t(if (Find.kind == "movie") "Find a movie" else "Find a TV show"),
                    ImeAction.Search, onAction = { Find.findTitles() }, onClear = { Find.clear() })
                Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Chip(L10n.t("Movies"), Find.kind == "movie") { Find.kind = "movie"; if (Find.query.isNotBlank()) search() }
                    Chip(L10n.t("TV Shows"), Find.kind == "tv") { Find.kind = "tv"; if (Find.query.isNotBlank()) search() }
                    Chip(L10n.t(if (Find.china) "China" else "World"), false) { Find.china = !Find.china; if (Find.chosen != null) Find.searchTitle() else if (Find.results != null) Find.searchText() }
                }
                Find.chosen?.let { title ->
                    Words(listOf(name(title), title.optString("year")).filter { it.isNotEmpty() }.joinToString(" · "), 17.sp, FontWeight.Bold)
                    if (title.optString("kind") == "tv") {
                        Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            Chip(L10n.t("Full seasons"), !Find.episodes) { Find.episodes = false; Find.searchTitle() }
                            Chip(L10n.t("Single episodes"), Find.episodes) { Find.episodes = true; if (Find.season == 0) Find.season = Find.seasons.lastOrNull() ?: 0; Find.searchTitle() }
                        }
                        if (Find.seasons.isNotEmpty()) Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            if (!Find.episodes) Chip(L10n.t("All seasons"), Find.season == 0) { Find.season = 0; Find.searchTitle() }
                            for (number in Find.seasons) Chip(String.format(L10n.locale(), L10n.t("Season %d"), number), Find.season == number) { Find.season = number; Find.searchTitle() }
                        }
                    }
                }
                if (Find.results != null) Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Chip(L10n.t("Most seeds"), !Find.bySize) { Find.bySize = false }
                    Chip(L10n.t("Largest first"), Find.bySize) { Find.bySize = true }
                }
                if (Find.note.isNotEmpty()) Words(Find.note, 13.sp, color = Ink.muted)
                Spacer(Modifier.height(4.dp))
            }
        }
        items(Find.titles, key = { it.optString("id") }) { TitleResult(it) { Find.choose(it) } }
        if (Find.titles.isNotEmpty()) item {
            Row(Modifier.fillMaxWidth().clickable { Find.searchText() }.padding(vertical = 14.dp)) {
                Words(String.format(L10n.locale(), L10n.t("Search torrents for “%s”"), Find.query.trim()), 15.sp, FontWeight.SemiBold)
            }
        }
        items(Find.ordered(), key = { it.optString("info_hash").ifEmpty { it.optString("name") } + it.optString("magnet").hashCode() }) { ReleaseRow(it) { openRelease(it) } }
    }
}

private fun name(title: JSONObject): String =
    if (L10n.locale().language == "zh" && title.optString("title_zh").isNotEmpty()) title.optString("title_zh") else title.optString("title")

/** A TMDB title the search found, with the poster it sent along. */
@Composable
private fun TitleResult(title: JSONObject, onClick: () -> Unit) {
    val poster = remember(title.optString("id")) {
        runCatching { Base64.decode(title.optString("poster"), Base64.DEFAULT).let { BitmapFactory.decodeByteArray(it, 0, it.size) } }.getOrNull()
    }
    Row(Modifier.fillMaxWidth().clickable(onClick = onClick).padding(vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        Box(Modifier.width(52.dp).aspectRatio(2f / 3f).clip(RoundedCornerShape(6.dp)).background(Ink.panel)) {
            poster?.let { Image(it.asImageBitmap(), null, Modifier.fillMaxSize(), contentScale = ContentScale.Crop) }
        }
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Words(name(title), 16.sp, FontWeight.SemiBold, lines = 2)
            Words(listOf(title.optString("year"), if (title.isNull("country")) "" else title.optString("country")).filter { it.isNotEmpty() }.joinToString(" · "), 13.sp, color = Ink.muted)
        }
    }
}

@Composable
private fun ReleaseRow(item: JSONObject, onClick: () -> Unit) {
    val context = LocalContext.current
    val cloud = item.optJSONObject("cloud")
    Column(Modifier.fillMaxWidth().padding(vertical = 5.dp).clip(RoundedCornerShape(12.dp)).background(Ink.panel).clickable(onClick = onClick).padding(12.dp)) {
        Words(TorrentSearchActivity.title(item), 15.sp, FontWeight.SemiBold, lines = 2)
        TorrentSearchActivity.summary(context, item).takeIf { it.isNotEmpty() }?.let { Words(it, 13.sp, color = Ink.text) }
        val tags = if (cloud != null) TorrentSearchActivity.cloudQuality(context, cloud) else TorrentSearchActivity.tags(item)
        if (tags.isNotEmpty()) Words(tags, 12.sp, color = Ink.muted)
    }
}

/** One release: what it is, its files, and Add (to Transmission, or from a Quark share into the library). */
@Composable
fun ReleasePage(item: JSONObject) {
    val context = LocalContext.current
    val cloud = TorrentSearchActivity.isCloud(item)
    var detail by remember(item) { mutableStateOf<JSONObject?>(null) }
    var loading by remember(item) { mutableStateOf(!cloud) }
    var release by remember(item) { mutableStateOf(item) }
    var paused by remember(item) { mutableStateOf(false) }
    var state by remember(item) { mutableStateOf("") }
    var saveAs by remember(item) { mutableStateOf(TorrentSearchActivity.libraryName(TorrentSearchActivity.title(item))) }
    LaunchedEffect(item) { if (!cloud) Find.detail(item) { detail = it; loading = false } }
    val releases = detail?.optJSONArray("releases").list()
    val files = (releases.firstOrNull { it.optJSONObject("item")?.toString() == release.toString() }?.optJSONArray("files") ?: detail?.optJSONArray("files")).list()
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(start = 16.dp, end = 16.dp, bottom = bottomRoom())) {
        item {
            Words(TorrentSearchActivity.title(item), 20.sp, FontWeight.Bold)
            Spacer(Modifier.height(6.dp))
            Words(TorrentSearchActivity.summary(context, item), 13.sp, color = Ink.text)
            val tags = item.optJSONObject("cloud")?.let { TorrentSearchActivity.cloudQuality(context, it) } ?: TorrentSearchActivity.tags(item)
            if (tags.isNotEmpty()) { Spacer(Modifier.height(4.dp)); Words(tags, 12.sp, color = Ink.muted) }
            if (item.optString("name") != TorrentSearchActivity.title(item)) { Spacer(Modifier.height(6.dp)); Words(item.optString("name"), 12.sp, color = Ink.muted) }
            Spacer(Modifier.height(16.dp))
            if (cloud) {
                Box(Modifier.fillMaxWidth().height(42.dp).clip(RoundedCornerShape(10.dp)).background(Ink.control).padding(horizontal = 12.dp), contentAlignment = Alignment.CenterStart) {
                    BasicTextField(saveAs, { saveAs = it }, Modifier.fillMaxWidth(), singleLine = true, textStyle = TextStyle(color = Ink.text, fontSize = 15.sp), cursorBrush = SolidColor(Ink.text))
                }
                Spacer(Modifier.height(10.dp))
                Action(L10n.t(if (state == "added") "Added to downloads" else if (state == "adding") "Adding…" else "Add"), prominent = state.isEmpty(), modifier = Modifier.fillMaxWidth()) {
                    if (state.isEmpty() && saveAs.isNotBlank()) { state = "adding"; Find.addCloud(item, saveAs.trim()) { message -> state = if (message == null) "" else "added"; message?.let { Phone.problem = it } } }
                }
                Spacer(Modifier.height(8.dp))
                Words(L10n.t("Videos are saved to your Quark Drive, downloaded into your library, then removed from the drive."), 12.sp, color = Ink.muted)
            } else {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Action(L10n.t(if (state == "added") "Added" else if (state == "adding") "Adding…" else "Add torrent"), prominent = state.isEmpty(), modifier = Modifier.weight(1f)) {
                        if (state.isEmpty()) { state = "adding"; Torrents.add(release, paused) { message -> state = if (message == null) "" else "added"; message?.let { Phone.problem = it } } }
                    }
                    Spacer(Modifier.width(10.dp))
                    Chip(L10n.t("Add paused"), paused) { paused = !paused }
                }
            }
            if (releases.size > 1) {
                Spacer(Modifier.height(14.dp))
                for (choice in releases) choice.optJSONObject("item")?.let { option ->
                    Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(8.dp)).background(if (option.toString() == release.toString()) Ink.control else Ink.panel).clickable { release = option; state = "" }.padding(10.dp)) {
                        Words(option.optString("name"), 13.sp, lines = 2)
                    }
                    Spacer(Modifier.height(6.dp))
                }
            }
            detail?.optString("description")?.takeIf { it.isNotBlank() }?.let { Spacer(Modifier.height(14.dp)); Words(it.trim(), 13.sp, color = Ink.text.copy(alpha = 0.85f)) }
            Spacer(Modifier.height(16.dp))
            if (!cloud) Words(when {
                loading -> L10n.t("Loading details…")
                detail == null -> L10n.t("Couldn’t load source details. You can still add this torrent.")
                else -> String.format(L10n.locale(), L10n.t("%d files"), files.size)
            }, 14.sp, FontWeight.Bold)
            Spacer(Modifier.height(6.dp))
        }
        items(files.size) { index ->
            val f = files[index]
            Column(Modifier.fillMaxWidth().padding(vertical = 5.dp)) {
                Words(f.optString("name").substringAfterLast('/'), 13.sp, lines = 2)
                if (!f.isNull("size_bytes")) Words(android.text.format.Formatter.formatShortFileSize(context, f.optLong("size_bytes")), 11.sp, color = Ink.muted)
            }
        }
    }
}
