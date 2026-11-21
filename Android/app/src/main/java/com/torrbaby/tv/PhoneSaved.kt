package com.torrbaby.tv

import android.Manifest
import android.app.Activity
import android.content.pm.PackageManager
import android.graphics.BitmapFactory
import android.os.Build
import android.text.format.Formatter
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

private val SavedGreen = Color(0xFF30D158)

/** Names in the interface's language from the library when it is here; as saved when offline. */
private val SavedItem.shownTitle get() = Phone.library?.find(titleId)?.title ?: title
private val SavedItem.shownHeading: String get() {
    val t = Phone.library?.find(titleId) ?: return heading.ifEmpty { label }
    return t.assets.firstOrNull { it.id == assetId }?.heading(t) ?: heading.ifEmpty { label }
}

/** Downloads (the iPhone's): what is on this phone, what is still coming, and the space it takes. */
@Composable
internal fun SavedPage(activity: Activity) {
    val context = LocalContext.current
    var acting by remember { mutableStateOf<SavedItem?>(null) }
    val items = Saved.items.toList().asReversed()
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(start = 16.dp, end = 16.dp, top = 4.dp, bottom = bottomRoom(24))) {
        item {
            Words(String.format(L10n.locale(), L10n.t("%s used · %s free"), Formatter.formatShortFileSize(context, Saved.used),
                Formatter.formatShortFileSize(context, Saved.free)), 13.sp, color = Ink.muted)
            Spacer(Modifier.height(12.dp))
            if (Saved.waitingForWiFi) {
                Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(Ink.panel).padding(14.dp), verticalAlignment = Alignment.CenterVertically) {
                    Words(L10n.t("Downloads are waiting for Wi-Fi"), 14.sp, modifier = Modifier.weight(1f))
                    Spacer(Modifier.width(10.dp))
                    Action(L10n.t("Allow cellular for 3 hours")) { Saved.allowMeteredForAWhile() }
                }
                Spacer(Modifier.height(12.dp))
            }
            if (items.isEmpty()) Words(L10n.t("Nothing downloaded yet"), 15.sp, color = Ink.muted, modifier = Modifier.padding(top = 40.dp).fillMaxWidth())
        }
        items(items, key = { it.id }) { item ->
            SavedRow(item, onPlay = { Phone.playSaved(activity, item, false) }, onMore = { acting = item })
        }
    }
    acting?.let { item -> SavedActions(item) { acting = null } }
}

@Composable
private fun SavedRow(item: SavedItem, onPlay: () -> Unit, onMore: () -> Unit) {
    val context = LocalContext.current
    val received = Saved.live?.takeIf { it.first == item.id }?.second ?: item.received
    val fraction = if (item.size > 0) (received.toFloat() / item.size).coerceIn(0f, 1f) else 0f
    Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).clickable { if (item.state == "ready") onPlay() else onMore() }.padding(vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically) {
        Poster(item, Modifier.width(56.dp).aspectRatio(2f / 3f).clip(RoundedCornerShape(6.dp)))
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Words(item.shownTitle, 16.sp, FontWeight.SemiBold, lines = 1)
            if (item.hasSeason) Words(item.shownHeading, 13.sp, color = Ink.muted, lines = 1)
            Spacer(Modifier.height(3.dp))
            val size = Formatter.formatShortFileSize(context, item.size)
            Words(when (item.state) {
                "ready" -> if (item.watched) size + " · " + L10n.t("Watched") else size
                "preparing" -> L10n.t("Preparing…")
                "queued" -> L10n.t("Queued")
                "waiting" -> L10n.t(if (Saved.waitingForWiFi) "Waiting for Wi-Fi" else "Waiting for network")
                "paused" -> L10n.t("Paused") + " · " + String.format(L10n.locale(), L10n.t("%s of %s"), Formatter.formatShortFileSize(context, received), size)
                "failed" -> L10n.t("Download failed") + (if (item.error.isNotEmpty()) " · " + item.error else "")
                else -> String.format(L10n.locale(), L10n.t("%s of %s"), Formatter.formatShortFileSize(context, received), size)
            }, 12.sp, color = if (item.state == "failed") Color(0xFFFF6B6B) else Ink.muted, lines = 2)
            if (item.state != "ready" && item.size > 0) {
                Spacer(Modifier.height(6.dp))
                Box(Modifier.fillMaxWidth().height(4.dp).clip(RoundedCornerShape(2.dp)).background(Ink.control)) {
                    Box(Modifier.fillMaxWidth(fraction).fillMaxHeight().background(if (item.state == "downloading") SavedGreen else Ink.muted))
                }
            }
        }
        Spacer(Modifier.width(8.dp))
        Box(Modifier.size(40.dp).clip(CircleShape).clickable(onClick = onMore), contentAlignment = Alignment.Center) { Words("⋯", 22.sp, FontWeight.Bold, Ink.muted) }
    }
}

@Composable
private fun Poster(item: SavedItem, modifier: Modifier) {
    val file = Saved.poster(item)
    val picture = remember(item.id, file.exists()) { if (file.exists()) BitmapFactory.decodeFile(file.path)?.asImageBitmap() else null }
    if (picture != null) Image(picture, null, modifier, contentScale = ContentScale.Crop)
    else Box(modifier.background(Ink.control), contentAlignment = Alignment.Center) { Symbol(Glyph.Film, 22.dp, Ink.muted) }
}

/** Play, pause or resume, and remove a download (the alert the ⋯ opens). */
@Composable
private fun SavedActions(item: SavedItem, close: () -> Unit) {
    val activity = LocalContext.current as Activity
    BackHandler(onBack = close)
    Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.6f)).clickable(onClick = close), contentAlignment = Alignment.Center) {
        Column(Modifier.fillMaxWidth(0.82f).clip(RoundedCornerShape(16.dp)).background(Ink.panel).clickable(enabled = false) {}.padding(18.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Words(item.shownTitle + (if (item.hasSeason) " · " + item.shownHeading else ""), 17.sp, FontWeight.Bold, lines = 2)
            Spacer(Modifier.height(4.dp))
            when (item.state) {
                "ready" -> {
                    Action(L10n.t(if (item.position > 0) "Resume" else "Play"), prominent = true, modifier = Modifier.fillMaxWidth()) { close(); Phone.playSaved(activity, item, false) }
                    if (item.position > 0) Action(L10n.t("Start from Beginning"), modifier = Modifier.fillMaxWidth()) { close(); Phone.playSaved(activity, item, true) }
                }
                "queued", "downloading", "waiting" -> Action(L10n.t("Pause download"), modifier = Modifier.fillMaxWidth()) { Saved.pause(item.id); close() }
                "paused", "failed" -> Action(L10n.t("Resume download"), prominent = true, modifier = Modifier.fillMaxWidth()) {
                    if (item.size > 0) Saved.resume(item.id) else Phone.library?.find(item.titleId)?.let { t -> t.assets.firstOrNull { it.id == item.assetId }?.let { Saved.add(t, it) } }
                    close()
                }
            }
            Box(Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).background(Color(0xFF5A1D1D)).clickable { Saved.remove(item.id); close() }.padding(vertical = 11.dp),
                contentAlignment = Alignment.Center) { Words(L10n.t("Remove download"), 15.sp, FontWeight.SemiBold, Color(0xFFFF6B6B)) }
            Action(L10n.t("Cancel"), modifier = Modifier.fillMaxWidth()) { close() }
        }
    }
}

/** Ask once for the notification that shows a download's progress (Android 13+). */
private fun askToNotify(activity: Activity) {
    if (Build.VERSION.SDK_INT >= 33 && activity.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED)
        activity.requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 3)
}

/** The title page's Download (beside Play): its progress while it comes, then Downloaded. */
@Composable
internal fun SaveButton(title: Library.Title, asset: Library.Asset, openSaved: () -> Unit) {
    val activity = LocalContext.current as Activity
    val copy = Saved.items.firstOrNull { it.assetId == asset.id }
    val received = Saved.live?.takeIf { it.first == copy?.id }?.second ?: copy?.received ?: 0
    val label = when (copy?.state) {
        null, "failed" -> L10n.t("Download")
        "ready" -> L10n.t("Downloaded")
        "downloading" -> if (copy.size > 0) "${(received * 100 / copy.size)}%" else L10n.t("Downloading")
        "preparing" -> L10n.t("Preparing…")
        "paused" -> L10n.t("Paused")
        else -> L10n.t("Queued")
    }
    Row(Modifier.clip(RoundedCornerShape(10.dp)).background(Ink.control).clickable {
        if (copy == null || copy.state == "failed") { askToNotify(activity); Saved.add(title, asset) } else openSaved()
    }.padding(horizontal = 14.dp, vertical = 11.dp), verticalAlignment = Alignment.CenterVertically) {
        Symbol(if (copy?.state == "ready") Glyph.Check else Glyph.Save, 18.dp, if (copy?.state == "ready") SavedGreen else Ink.text)
        Spacer(Modifier.width(6.dp))
        Words(label, 15.sp, FontWeight.SemiBold, lines = 1)
    }
}

/** An episode row's download mark: tap to save it; its progress or a check once it is here. */
@Composable
internal fun SaveMark(title: Library.Title, asset: Library.Asset, openSaved: () -> Unit) {
    val activity = LocalContext.current as Activity
    val copy = Saved.items.firstOrNull { it.assetId == asset.id }
    val received = Saved.live?.takeIf { it.first == copy?.id }?.second ?: copy?.received ?: 0
    Box(Modifier.size(40.dp).clip(CircleShape).clickable(enabled = asset.complete) {
        if (copy == null || copy.state == "failed") { askToNotify(activity); Saved.add(title, asset) } else openSaved()
    }, contentAlignment = Alignment.Center) {
        when (copy?.state) {
            null, "failed" -> Symbol(Glyph.Save, 20.dp, if (asset.complete) Ink.muted else Ink.control)
            "ready" -> Symbol(Glyph.Check, 20.dp, SavedGreen)
            "downloading" -> Words(if (copy.size > 0) "${received * 100 / copy.size}%" else "…", 11.sp, FontWeight.Bold, SavedGreen)
            else -> Words("…", 15.sp, FontWeight.Bold, Ink.muted)
        }
    }
}
