package com.torrbaby.tv

import android.app.Activity
import android.content.Context
import android.graphics.Bitmap
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicText
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.FilterQuality
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.json.JSONObject

/** The iPhone app's colours (Apple/Torrbaby/App.swift Theme). */
private object Ink {
    val canvas = Color.Black
    val sidebar = Color(0xFF0E0E0E)
    val panel = Color(0xFF171717)
    val control = Color(0xFF292929)
    val text = Color.White
    val muted = Color(0xFFA6A6A6)
    val brand = Color(0xFFB36EFF)
    val green = Color(0xFF34C759)
}

private sealed interface Page {
    data object Home : Page
    data object Movies : Page
    data object Shows : Page
    data object Settings : Page
    data class Title(val id: String) : Page
}

@Composable
private fun Words(text: String, size: TextUnit = 15.sp, weight: FontWeight = FontWeight.Normal, color: Color = Ink.text,
                  lines: Int = Int.MAX_VALUE, modifier: Modifier = Modifier) =
    BasicText(text, modifier, TextStyle(color = color, fontSize = size, fontWeight = weight), overflow = TextOverflow.Ellipsis, maxLines = lines)

@Composable
private fun Action(label: String, prominent: Boolean = false, modifier: Modifier = Modifier, onClick: () -> Unit) {
    Box(modifier.clip(RoundedCornerShape(10.dp)).background(if (prominent) Ink.text else Ink.control).clickable(onClick = onClick)
        .padding(horizontal = 16.dp, vertical = 11.dp), contentAlignment = Alignment.Center) {
        Words(label, 15.sp, FontWeight.SemiBold, if (prominent) Color.Black else Ink.text, 1)
    }
}

@Composable
fun PhoneScreen(activity: Activity) {
    Box(Modifier.fillMaxSize().background(Ink.canvas)) {
        when (Phone.stage) {
            Stage.Starting -> Waiting(L10n.t("Connecting…"))
            Stage.Setup -> Setup()
            Stage.Pairing -> Pairing()
            Stage.Profiles -> Profiles()
            Stage.Ready -> Browser(activity)
        }
        Phone.problem?.let { Problem(it) }
    }
}

@Composable
private fun Waiting(text: String) {
    Column(Modifier.fillMaxSize().systemBarsPadding().padding(24.dp), verticalArrangement = Arrangement.Center, horizontalAlignment = Alignment.CenterHorizontally) {
        Words("torrbaby", 34.sp, FontWeight.Bold, Ink.brand)
        Spacer(Modifier.height(16.dp))
        Words(text, 15.sp, color = Ink.muted)
    }
}

/** An error as a banner at the bottom, with Try again. */
@Composable
private fun BoxScope.Problem(text: String) {
    Row(Modifier.align(Alignment.BottomCenter).navigationBarsPadding().padding(12.dp).fillMaxWidth()
        .clip(RoundedCornerShape(12.dp)).background(Color(0xFF3A1D1D)).padding(14.dp), verticalAlignment = Alignment.CenterVertically) {
        Words(text, 14.sp, modifier = Modifier.weight(1f))
        Spacer(Modifier.width(12.dp))
        Action(L10n.t("Try again")) { Phone.problem = null; if (Phone.stage == Stage.Setup) Phone.findHome() else Phone.start() }
    }
}

@Composable
private fun Setup() {
    Column(Modifier.fillMaxSize().systemBarsPadding().padding(24.dp), verticalArrangement = Arrangement.Center) {
        Words("torrbaby", 34.sp, FontWeight.Bold, Ink.brand)
        Spacer(Modifier.height(24.dp))
        Words(L10n.t(if (Phone.searching) "Looking for your server on this network…" else "No server found on this network."), 16.sp, color = Ink.muted)
        Spacer(Modifier.height(28.dp))
        Action(L10n.t("Set up away from home"), prominent = true, modifier = Modifier.fillMaxWidth()) { Phone.pairAway() }
        Spacer(Modifier.height(12.dp))
        if (!Phone.searching) Action(L10n.t("Search again"), modifier = Modifier.fillMaxWidth()) { Phone.findHome() }
    }
}

@Composable
private fun Pairing() {
    BackHandler { Phone.cancelPairing() }
    Column(Modifier.fillMaxSize().systemBarsPadding().verticalScroll(rememberScrollState()).padding(24.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        Words(L10n.t("Set up away from home"), 24.sp, FontWeight.Bold)
        Spacer(Modifier.height(12.dp))
        Words(L10n.t("Scan this code with Torrbaby on a phone or tablet that is already connected to your server."), 15.sp, color = Ink.muted)
        Spacer(Modifier.height(20.dp))
        Phone.pairingCode?.let {
            Image(it.asImageBitmap(), L10n.t("Setup code"), Modifier.fillMaxWidth().aspectRatio(1f).clip(RoundedCornerShape(12.dp)).background(Color.White).padding(12.dp), filterQuality = FilterQuality.None)
        }
        Spacer(Modifier.height(16.dp))
        Words(Phone.pairingStatus, 14.sp, color = Ink.muted)
        Spacer(Modifier.height(20.dp))
        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Action(L10n.t("Back")) { Phone.cancelPairing() }
            Action(L10n.t("New setup code")) { Phone.pairAway() }
        }
    }
}

@Composable
private fun Profiles() {
    Column(Modifier.fillMaxSize().systemBarsPadding().padding(24.dp), verticalArrangement = Arrangement.Center) {
        Words(L10n.t("Who's watching?"), 26.sp, FontWeight.Bold)
        Spacer(Modifier.height(24.dp))
        if (Phone.profiles.isEmpty()) Words(L10n.t("Loading…"), color = Ink.muted)
        for (person in Phone.profiles) {
            Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).clickable { Phone.activate(person, true) }.padding(vertical = 10.dp),
                verticalAlignment = Alignment.CenterVertically) {
                Avatar(person, 48)
                Spacer(Modifier.width(14.dp))
                Words(person.optString("name"), 18.sp, FontWeight.SemiBold)
            }
        }
    }
}

@Composable
private fun Avatar(person: JSONObject?, size: Int) {
    val background = runCatching { Color(android.graphics.Color.parseColor(person?.optString("background_color"))) }.getOrDefault(Ink.brand)
    val foreground = runCatching { Color(android.graphics.Color.parseColor(person?.optString("text_color"))) }.getOrDefault(Color.White)
    Box(Modifier.size(size.dp).clip(CircleShape).background(background), contentAlignment = Alignment.Center) {
        Words(ProfileUi.firstCharacter(person?.optString("name") ?: ""), (size * 0.45).sp, FontWeight.Bold, foreground)
    }
}

// Browsing -----------------------------------------------------------------------------------

@Composable
private fun Browser(activity: Activity) {
    val pages = remember { mutableStateListOf<Page>(Page.Home) }
    var menu by remember { mutableStateOf(false) }
    BackHandler(enabled = menu || pages.size > 1) { if (menu) menu = false else pages.removeAt(pages.lastIndex) }
    val page = pages.last()
    Column(Modifier.fillMaxSize()) {
        Row(Modifier.fillMaxWidth().statusBarsPadding().height(52.dp).padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.size(44.dp).clip(CircleShape).clickable { if (pages.size > 1) pages.removeAt(pages.lastIndex) else menu = true }, contentAlignment = Alignment.Center) {
                Words(if (pages.size > 1) "‹" else "☰", if (pages.size > 1) 30.sp else 22.sp)
            }
            Spacer(Modifier.width(4.dp))
            Words(when (page) {
                Page.Home -> L10n.t("Home"); Page.Movies -> L10n.t("Movies"); Page.Shows -> L10n.t("TV Shows")
                Page.Settings -> L10n.t("Settings"); is Page.Title -> ""
            }, 20.sp, FontWeight.Bold, lines = 1)
        }
        Box(Modifier.weight(1f)) {
            val open = { id: String -> pages.add(Page.Title(id)); Unit }
            when (page) {
                Page.Home -> Home(open)
                Page.Movies -> Grid("movie", open)
                Page.Shows -> Grid("tv", open)
                Page.Settings -> Settings(activity)
                is Page.Title -> TitlePage(activity, page.id)
            }
        }
    }
    if (menu) Menu(pages.first()) { chosen -> pages.clear(); pages.add(chosen); menu = false }
}

/** The iPhone's menu: the library's sections, Settings, an update to install, and how it is connected. */
@Composable
private fun Menu(current: Page, choose: (Page) -> Unit) {
    val activity = LocalContext.current as Activity
    Column(Modifier.fillMaxSize().background(Ink.sidebar).systemBarsPadding().padding(16.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Words("tb", 22.sp, FontWeight.Bold, Ink.brand); Spacer(Modifier.width(8.dp)); Words("torrbaby", 20.sp, FontWeight.Bold)
        }
        Spacer(Modifier.height(20.dp))
        Words(L10n.t("Library"), 13.sp, FontWeight.SemiBold, Ink.muted)
        Spacer(Modifier.height(6.dp))
        for ((page, label) in listOf(Page.Home to "Home", Page.Movies to "Movies", Page.Shows to "TV Shows")) MenuItem(L10n.t(label), page == current) { choose(page) }
        Spacer(Modifier.height(14.dp))
        Words(L10n.t("Server"), 13.sp, FontWeight.SemiBold, Ink.muted)
        Spacer(Modifier.height(6.dp))
        MenuItem(L10n.t("Settings"), current == Page.Settings) { choose(Page.Settings) }
        Spacer(Modifier.weight(1f))
        Phone.update?.let { Action(String.format(L10n.locale(), L10n.t("Install %s"), it.optString("build")), prominent = true, modifier = Modifier.fillMaxWidth()) { Phone.installUpdate(activity) }; Spacer(Modifier.height(10.dp)) }
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.Center, verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.size(7.dp).clip(CircleShape).background(Ink.green)); Spacer(Modifier.width(7.dp))
            Words(Phone.connectionLabel, 12.sp, FontWeight.SemiBold)
        }
    }
}

@Composable
private fun MenuItem(label: String, selected: Boolean, onClick: () -> Unit) {
    Box(Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).background(if (selected) Ink.text else Color.Transparent).clickable(onClick = onClick).padding(horizontal = 14.dp, vertical = 12.dp)) {
        Words(label, 17.sp, FontWeight.SemiBold, if (selected) Color.Black else Ink.text)
    }
}

/** Room at the end of a list for Android's navigation bar, which the pages draw under. */
@Composable
private fun bottomRoom(extra: Int = 24) = WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding() + extra.dp

/** A title's artwork from Diamond (Art's caches), grey until it arrives. */
@Composable
private fun Artwork(id: String, variant: String, revision: String, modifier: Modifier) {
    var picture by remember(id, variant, revision) { mutableStateOf<Bitmap?>(null) }
    val art = Phone.art
    LaunchedEffect(id, variant, revision, art) { if (revision.isNotEmpty()) art?.bitmap(id, variant, revision) { picture = it } }
    Box(modifier.background(Ink.panel)) {
        picture?.let { Image(it.asImageBitmap(), null, Modifier.fillMaxSize(), contentScale = ContentScale.Crop) }
    }
}

@Composable
private fun Poster(title: Library.Title, open: (String) -> Unit, modifier: Modifier = Modifier) {
    Column(modifier.clickable { open(title.id) }) {
        Box {
            Artwork(title.id, "poster", title.poster, Modifier.fillMaxWidth().aspectRatio(2f / 3f).clip(RoundedCornerShape(8.dp)))
            if (title.watched()) Box(Modifier.align(Alignment.TopEnd).padding(5.dp).size(20.dp).clip(CircleShape).background(Color.Black.copy(alpha = 0.7f)), contentAlignment = Alignment.Center) { Words("✓", 12.sp, FontWeight.Bold) }
        }
        Spacer(Modifier.height(5.dp))
        Words(title.title, 13.sp, FontWeight.Medium, lines = 1)
    }
}

@Composable
private fun Home(open: (String) -> Unit) {
    val library = Phone.library ?: return Waiting(L10n.t("Loading library…"))
    val recent = remember(library) { library.filter("", "", "", 0) }
    val hero = remember(library) { recent.firstOrNull { it.backdrop.isNotEmpty() && it.playable().isNotEmpty() && !it.watched() } ?: recent.firstOrNull { it.backdrop.isNotEmpty() } }
    val shelves = remember(library) {
        buildList {
            add("Continue watching" to library.continuing())
            add("Recently added" to recent)
            add("Movies" to recent.filter { it.kind == "movie" })
            add("TV Shows" to recent.filter { it.kind == "tv" })
            add("Recently watched" to library.recentlyWatched().filter { it.watched() })
            for (genre in library.genres("")) recent.filter { genre in it.genres }.takeIf { it.size >= 7 }?.let { add(genre to it) }
        }.filter { it.second.isNotEmpty() }
    }
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = bottomRoom())) {
        hero?.let { title ->
            item {
                Box(Modifier.fillMaxWidth().height(240.dp).padding(horizontal = 12.dp).clip(RoundedCornerShape(12.dp)).clickable { open(title.id) }) {
                    Artwork(title.id, "backdrop", title.backdrop, Modifier.fillMaxSize())
                    Box(Modifier.fillMaxSize().background(Brush.verticalGradient(listOf(Color.Transparent, Color.Black.copy(alpha = 0.9f)))))
                    Column(Modifier.align(Alignment.BottomStart).padding(16.dp)) {
                        Words(title.title, 26.sp, FontWeight.Bold, lines = 2)
                        Words(listOf(title.yearLabel()).plus(title.genres.take(2)).filter { it.isNotEmpty() }.joinToString(" · "), 13.sp, color = Color.White.copy(alpha = 0.8f))
                    }
                }
            }
        }
        items(shelves, key = { it.first }) { (name, titles) ->
            Column(Modifier.padding(top = 18.dp)) {
                Words(L10n.t(name), 17.sp, FontWeight.Bold, modifier = Modifier.padding(horizontal = 12.dp))
                Spacer(Modifier.height(8.dp))
                LazyRow(contentPadding = PaddingValues(horizontal = 12.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    items(titles, key = { it.id }) { Poster(it, open, Modifier.width(112.dp)) }
                }
            }
        }
    }
}

/**
 * Movies or TV Shows: search, genre, sort and Hide watched above the first row, scrolled out of
 * view at first (pull down to reach them), as on the iPhone. Sort and Hide watched are remembered.
 */
@Composable
private fun Grid(kind: String, open: (String) -> Unit) {
    val library = Phone.library ?: return Waiting(L10n.t("Loading library…"))
    val context = LocalContext.current
    val preferences = remember { context.getSharedPreferences("library", Context.MODE_PRIVATE) }
    var search by remember(kind) { mutableStateOf("") }
    var genre by remember(kind) { mutableStateOf("") }
    var sort by remember { mutableIntStateOf(preferences.getInt("sort", 0)) }
    var hides by remember { mutableStateOf(preferences.getBoolean("hides_watched", false)) }
    var choosing by remember { mutableStateOf<String?>(null) }
    val remember = { preferences.edit().putInt("sort", sort).putBoolean("hides_watched", hides).apply() }
    val titles = library.filter(kind, search, genre, sort).filter { !hides || !it.watched() }
    val state = rememberLazyGridState(initialFirstVisibleItemIndex = 1)
    val sorts = listOf("Recently added", "Alphabetical", "Release date")
    LazyVerticalGrid(GridCells.Adaptive(104.dp), Modifier.fillMaxSize(), state, PaddingValues(start = 12.dp, top = 12.dp, end = 12.dp, bottom = bottomRoom(12)),
        horizontalArrangement = Arrangement.spacedBy(10.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
        item(span = { GridItemSpan(maxLineSpan) }) {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Box(Modifier.fillMaxWidth().height(40.dp).clip(RoundedCornerShape(10.dp)).background(Ink.control).padding(horizontal = 12.dp), contentAlignment = Alignment.CenterStart) {
                    if (search.isEmpty()) Words(L10n.t("Search movies and TV shows"), color = Ink.muted, lines = 1)
                    BasicTextField(search, { search = it }, Modifier.fillMaxWidth(), singleLine = true, textStyle = TextStyle(color = Ink.text, fontSize = 15.sp), cursorBrush = SolidColor(Ink.text))
                }
                Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Action(if (genre.isEmpty()) L10n.t("All genres") else genre) { choosing = "genre" }
                    Action(L10n.t(sorts[sort])) { choosing = "sort" }
                    Action(L10n.t("Hide watched"), prominent = hides) { hides = !hides; remember() }
                }
            }
        }
        if (titles.isEmpty()) item(span = { GridItemSpan(maxLineSpan) }) {
            Words(L10n.t(if (search.isNotEmpty()) "No titles match your search." else if (hides) "Everything here has been watched." else "No movies or shows yet."), color = Ink.muted, modifier = Modifier.padding(vertical = 40.dp))
        }
        items(titles, key = { it.id }) { Poster(it, open) }
    }
    choosing?.let { which ->
        val options = if (which == "genre") listOf("") + library.genres(kind) else sorts
        Choice(options.map { if (it.isEmpty()) L10n.t("All genres") else if (which == "sort") L10n.t(it) else it },
            if (which == "genre") options.indexOf(genre) else sort, { choosing = null }) { index ->
            if (which == "genre") genre = options[index] else { sort = index; remember() }
            choosing = null
        }
    }
}

/** A list to choose one from, over the page. */
@Composable
private fun Choice(labels: List<String>, selected: Int, dismiss: () -> Unit, choose: (Int) -> Unit) {
    BackHandler(onBack = dismiss)
    Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.6f)).clickable(onClick = dismiss), contentAlignment = Alignment.Center) {
        LazyColumn(Modifier.fillMaxWidth(0.8f).heightIn(max = 480.dp).clip(RoundedCornerShape(14.dp)).background(Ink.panel).padding(vertical = 6.dp)) {
            items(labels.size) { index ->
                Row(Modifier.fillMaxWidth().clickable { choose(index) }.padding(horizontal = 18.dp, vertical = 13.dp)) {
                    Words(labels[index], 16.sp, if (index == selected) FontWeight.Bold else FontWeight.Normal, modifier = Modifier.weight(1f))
                    if (index == selected) Words("✓", 16.sp, FontWeight.Bold)
                }
            }
        }
    }
}

@Composable
private fun TitlePage(activity: Activity, id: String) {
    val title = Phone.library?.find(id) ?: return Waiting(L10n.t("Loading library…"))
    val preferred = title.preferred()
    val episodes = title.assets.filter { it.hasSeason && !it.extra() }
    val seasons = episodes.map { it.season }.distinct()
    var season by remember(id) { mutableIntStateOf(preferred?.takeIf { it.hasSeason }?.season ?: seasons.firstOrNull() ?: 0) }
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = bottomRoom(32))) {
        item {
            Box(Modifier.fillMaxWidth().aspectRatio(16f / 9f)) {
                Artwork(title.id, "backdrop", title.backdrop, Modifier.fillMaxSize())
                Box(Modifier.fillMaxSize().background(Brush.verticalGradient(listOf(Color.Transparent, Ink.canvas))))
            }
            Column(Modifier.padding(horizontal = 16.dp)) {
                Words(title.title, 26.sp, FontWeight.Bold)
                Spacer(Modifier.height(4.dp))
                Words(listOf(title.yearLabel(), title.rating).plus(title.genres.take(3)).filter { it.isNotEmpty() }.joinToString(" · "), 13.sp, color = Ink.muted)
                Spacer(Modifier.height(16.dp))
                if (preferred != null) {
                    Action(preferred.playLabel(title), prominent = true, modifier = Modifier.fillMaxWidth()) { Phone.play(activity, title, preferred, false) }
                    if (preferred.position > 0) { Spacer(Modifier.height(10.dp)); Action(L10n.t("Start from Beginning"), modifier = Modifier.fillMaxWidth()) { Phone.play(activity, title, preferred, true) } }
                }
                if (title.summary.isNotEmpty()) { Spacer(Modifier.height(16.dp)); Words(title.summary, 14.sp, color = Color.White.copy(alpha = 0.85f)) }
                if (seasons.size > 1) {
                    Spacer(Modifier.height(18.dp))
                    Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        for (number in seasons) Action(String.format(L10n.locale(), L10n.t("Season %d"), number), prominent = number == season) { season = number }
                    }
                }
                Spacer(Modifier.height(10.dp))
            }
        }
        items(episodes.filter { it.season == season }, key = { it.id }) { episode ->
            Row(Modifier.fillMaxWidth().clickable(enabled = episode.complete) { Phone.play(activity, title, episode, false) }.padding(horizontal = 16.dp, vertical = 10.dp),
                verticalAlignment = Alignment.CenterVertically) {
                Artwork(episode.id, "episode", episode.thumbnail, Modifier.width(128.dp).aspectRatio(16f / 9f).clip(RoundedCornerShape(6.dp)))
                Spacer(Modifier.width(12.dp))
                Column(Modifier.weight(1f)) {
                    Words(episode.label(), 14.sp, FontWeight.SemiBold, lines = 2)
                    Words(episode.status(), 12.sp, color = Ink.muted, lines = 1)
                }
                if (episode.watched) Words("✓", 15.sp, FontWeight.Bold, Ink.muted)
            }
        }
    }
}

/** "Play", "Resume" or "Play S1 E3", as the TV's main button says it. */
private fun Library.Asset.playLabel(title: Library.Title): String =
    when {
        position > 0 -> L10n.t("Resume")
        hasSeason && title.kind == "tv" -> String.format(L10n.locale(), L10n.t("Play S%d E%d"), season, episode)
        else -> L10n.t("Play")
    }

@Composable
private fun Settings(activity: Activity) {
    val person = Phone.api?.profileAppearance
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp).navigationBarsPadding(), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(Ink.panel).padding(14.dp), verticalAlignment = Alignment.CenterVertically) {
            Avatar(person, 40); Spacer(Modifier.width(12.dp))
            Words(Phone.api?.profileName ?: "", 17.sp, FontWeight.SemiBold, modifier = Modifier.weight(1f))
            Action(L10n.t("Switch user")) { Phone.chooseProfile(true) }
        }
        Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(Ink.panel).padding(14.dp), verticalAlignment = Alignment.CenterVertically) {
            Words(L10n.t("Version"), 16.sp, modifier = Modifier.weight(1f))
            Words(BuildConfig.VERSION_NAME + " (" + BuildConfig.VERSION_CODE + ")", 15.sp, color = Ink.muted)
            Phone.update?.let { Spacer(Modifier.width(10.dp)); Action(String.format(L10n.locale(), L10n.t("Install %s"), it.optString("build")), prominent = true) { Phone.installUpdate(activity) } }
        }
        Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(Ink.panel).padding(14.dp), verticalAlignment = Alignment.CenterVertically) {
            Words(L10n.t("Connection"), 16.sp, modifier = Modifier.weight(1f))
            Words(Phone.connectionLabel, 15.sp, color = Ink.muted)
        }
        Action(L10n.t("Disconnect this phone"), modifier = Modifier.fillMaxWidth()) { Phone.disconnect() }
    }
}
