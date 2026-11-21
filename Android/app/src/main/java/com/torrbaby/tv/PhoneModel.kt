package com.torrbaby.tv

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.os.Build
import android.os.Handler
import android.os.Looper
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import org.json.JSONObject
import java.util.concurrent.Executors

/** Where the phone app is: finding the server, linking, choosing who is watching, or browsing. */
enum class Stage { Starting, Setup, Pairing, Profiles, Ready }

/**
 * The phone and tablet app's state, over the same connection, library and player code as the TV
 * (Api, SecureStore, ServerRoute, Relay, Pairing, Library, Art, PlaybackActivity). Network work runs
 * on one background thread; state changes on the main thread, where Compose reads them.
 */
object Phone {
    private val io = Executors.newSingleThreadExecutor()
    private val main = Handler(Looper.getMainLooper())
    private lateinit var app: Context

    var stage by mutableStateOf(Stage.Starting)
    var api by mutableStateOf<Api?>(null)
        private set
    var art by mutableStateOf<Art?>(null)
        private set
    var library by mutableStateOf<Library?>(null)
        private set
    /** Why the server cannot be used now (shown with Try again), or null. */
    var problem by mutableStateOf<String?>(null)
    var searching by mutableStateOf(false)
    var profiles by mutableStateOf<List<JSONObject>>(emptyList())
    var pairingCode by mutableStateOf<Bitmap?>(null)
    var pairingStatus by mutableStateOf("")
    var update by mutableStateOf<JSONObject?>(null)
    /** Bumped by each new linking attempt; an older one's thread stops. */
    private var generation = 0

    fun attach(context: Context) {
        if (::app.isInitialized) return
        app = context.applicationContext
        Api.platform = "Android"
        Relay.watchNetwork(app)
        start()
    }

    private fun post(action: () -> Unit) { main.post(action) }

    /** From the saved link: find the server (at home, else through the relay), then the library. */
    fun start() {
        problem = null
        val saved = SecureStore(app).load()
        if (saved == null) { stage = Stage.Setup; findHome(); return }
        stage = if (library == null) Stage.Starting else stage
        io.execute {
            val base = ServerRoute.locate(app, saved.client, saved.token, 8000)
            post {
                if (base.isEmpty()) { problem = L10n.t("Your server is not answering. Check this phone’s internet connection."); return@post }
                use(Api(base, saved.token, saved.client, saved.profile, saved.profileName).also { it.profileAppearance = saved.profileAppearance })
                if (!Relay.isRelay(base)) learnAtHome(base)
                if (saved.profile.isEmpty()) chooseProfile(false) else { stage = Stage.Ready; refresh() }
            }
        }
    }

    private fun use(next: Api) {
        ServerRoute.found = next.base
        if (api?.base != next.base || api?.profile != next.profile) { art?.close(); art = Art(app, next) }
        api = next
    }

    val connectionLabel: String
        get() = api?.let { String.format(L10n.t("Connected · %s"), if (Relay.isRelay(it.base)) "TB" else "LAN") } ?: L10n.t("Offline")

    // Linking ---------------------------------------------------------------------------------

    /** On the home network the server is found and this phone registers itself, nothing to type. */
    fun findHome() {
        searching = true
        io.execute {
            val home = ServerRoute.find(app, 5000)
            if (home.isEmpty()) { post { searching = false }; return@execute }
            try {
                val credential = Api(home, "", "").request("POST", "/v1/lan/register", JSONObject().put("name", Build.MODEL).put("platform", Api.platform))
                val linked = Api(home, credential.getString("token"), credential.getString("client_id"))
                post { searching = false; linked(linked) }
            } catch (e: Exception) {
                post {
                    searching = false
                    problem = if (e is Api.AuthException) L10n.t("This server has not enabled automatic connection for this network.") else MainActivity.message(e)
                }
            }
        }
    }

    private fun linked(next: Api) {
        SecureStore(app).save(next)
        use(next)
        if (!Relay.isRelay(next.base)) learnAtHome(next.base)
        chooseProfile(false)
    }

    /** At home, Diamond says who it is and where its relay is: kept, so it is found away too. */
    private fun learnAtHome(home: String) {
        val credentials = api ?: return
        io.execute {
            Relay.stop()
            try {
                val server = Api(home, credentials.token, credentials.client).request("GET", "/v1/ping", null).optJSONObject("server")
                if (server != null) Relay.Remote.from(server.optString("identity"), server.optJSONObject("relay")).save(app)
            } catch (_: Exception) { }
        }
    }

    /**
     * Away from home (docs/remote-pairing.md): this phone shows a code that Torrbaby on a linked
     * iPhone, iPad or Mac scans, then claims its setup from Diamond through the relay.
     */
    fun pairAway() {
        val attempt = ++generation
        stage = Stage.Pairing
        val pairing = try { Pairing.fresh() } catch (e: Exception) { problem = MainActivity.message(e); return }
        pairingCode = try { QrCode.bitmap(pairing.code(Build.MODEL, Api.platform), 720) } catch (e: Exception) { null }
        pairingStatus = L10n.t("Connecting to Torrbaby’s relay…")
        val current = { attempt == generation && stage == Stage.Pairing }
        val say = { text: String -> post { if (current()) pairingStatus = text } }
        Thread({
            val deadline = System.currentTimeMillis() + Pairing.LIFETIME_MILLIS
            var relay = Relay.base(Relay.DEFAULT_ADDRESS, Relay.DEFAULT_FINGERPRINT, "", "", "")
            if (relay.isEmpty()) { say(L10n.t("Could not reach Torrbaby’s relay. Check this phone’s internet connection, then choose New setup code.")); return@Thread }
            say(L10n.t("Waiting for the code to be scanned…"))
            while (current() && System.currentTimeMillis() < deadline) {
                try {
                    val setup = pairing.open(Api(relay, "", "").request("POST", "/v1/pairings/remote/claim", pairing.claim()))
                    if (setup != null) { finishPairing(setup, attempt, say); return@Thread }
                } catch (_: java.security.GeneralSecurityException) {
                } catch (_: SecurityException) {
                } catch (e: Exception) {
                    say(L10n.t("Your server is not answering through the relay yet. Still trying…"))
                    Relay.base(Relay.DEFAULT_ADDRESS, Relay.DEFAULT_FINGERPRINT, "", "", "").takeIf { it.isNotEmpty() }?.let { relay = it }
                }
                try { Thread.sleep(2000) } catch (_: InterruptedException) { return@Thread }
            }
            say(L10n.t("This code has expired. Choose New setup code to show another."))
        }, "Pairing").start()
    }

    private fun finishPairing(setup: JSONObject, attempt: Int, say: (String) -> Unit) {
        say(L10n.t("Setting up…"))
        val remote = Relay.Remote.from(setup.optJSONObject("server")?.optString("identity") ?: "", setup.optJSONObject("relay"))
        if (remote.identity.isEmpty()) { say(L10n.t("Your server did not send its identity. Update Torrbaby on the server, then choose New setup code.")); return }
        remote.save(app)
        val client = setup.optString("client_id")
        val base = Relay.base(remote.relayAddress, remote.relayFingerprint, remote.identity, client, setup.optString("token"))
        if (base.isEmpty()) { say(L10n.t("Could not reach Torrbaby’s relay. Check this phone’s internet connection, then choose New setup code.")); return }
        post { if (attempt == generation) linked(Api(base, setup.optString("token"), client)) }
    }

    fun cancelPairing() { generation++; stage = Stage.Setup; findHome() }

    fun disconnect() {
        generation++
        SecureStore(app).clear()
        Relay.stop()
        art?.close(); art = null
        api = null; library = null; update = null
        stage = Stage.Setup
        findHome()
    }

    // Profiles --------------------------------------------------------------------------------

    fun chooseProfile(switching: Boolean) {
        val current = api ?: return
        stage = Stage.Profiles
        profiles = emptyList()
        io.execute {
            try {
                current.register()
                val state = current.request("GET", "/v1/devices/" + Api.part(current.client) + "/profile", null)
                val people = state.getJSONArray("profiles").let { list -> (0 until list.length()).map { list.getJSONObject(it) } }
                val selected = state.optString("profile_id")
                val chosen = people.firstOrNull { it.optString("id") == selected }
                if (!switching && chosen != null) { activate(chosen, false); return@execute }
                post { profiles = people }
            } catch (e: Exception) { failed(e) }
        }
    }

    fun activate(person: JSONObject, persist: Boolean) {
        val current = api ?: return
        io.execute {
            try {
                val id = person.getString("id")
                if (persist) current.request("PUT", "/v1/devices/" + Api.part(current.client) + "/profile", JSONObject().put("profile_id", id))
                val next = Api(current.base, current.token, current.client, id, person.optString("name")).also { it.profileAppearance = person }
                SecureStore(app).save(next)
                post { library = null; use(next); stage = Stage.Ready; refresh() }
            } catch (e: Exception) { failed(e) }
        }
    }

    // Library ---------------------------------------------------------------------------------

    /** The cached library at once, then the server's when it has changed. */
    fun refresh() {
        val current = api ?: return
        io.execute {
            try {
                val cache = BrowseCache(app, current)
                var shown = library
                if (shown == null) cache.read("library", 0)?.let { raw ->
                    shown = Library.parse(CatalogJson.parse(String(raw, Charsets.UTF_8)))
                    val cached = shown
                    post { if (api === current) library = cached }
                }
                if (shown == null || current.libraryChanged()) {
                    val fresh = Library.parse(current.request("GET", "/v1/library", null))
                    cache.write("library", current.libraryRaw.toByteArray(Charsets.UTF_8))
                    shown = fresh
                }
                val hidden = current.request("GET", "/v1/library/progress", null).optJSONArray("hidden")?.let { ids -> (0 until ids.length()).map { ids.optString(it) }.toSet() } ?: emptySet()
                shown?.titles?.forEach { it.continueWatchingHidden = it.id in hidden }
                val result = shown
                post { if (api === current) { library = result; problem = null } }
            } catch (e: Exception) { failed(e) }
        }
        checkForUpdate()
        Torrents.refresh()
    }

    private fun failed(e: Exception) = post {
        if (e is Api.AuthException) { disconnect(); problem = L10n.t("The connection has expired or was revoked. Link this phone again."); return@post }
        problem = MainActivity.message(e)
    }

    // Playback --------------------------------------------------------------------------------

    fun play(activity: Activity, title: Library.Title, asset: Library.Asset, beginning: Boolean) {
        if (asset.offline) { problem = L10n.t("This title is on a drive that is not connected"); return }
        if (!asset.complete) { problem = L10n.t("This media file is not yet available on the server."); return }
        val ids = ArrayList<String>(); val names = ArrayList<String>(); val headings = ArrayList<String>()
        val queued = HashSet<String>()
        var next = title.next(asset.id)
        while (next != null && queued.add(next.id)) { ids.add(next.id); names.add(next.label()); headings.add(next.heading(title)); next = title.next(next.id) }
        activity.startActivity(Intent(activity, PlaybackActivity::class.java)
            .putExtra("asset", asset.id).putExtra("title_id", title.id).putExtra("title", title.title)
            .putExtra("name", asset.label()).putExtra("heading", asset.heading(title)).putExtra("beginning", beginning)
            .putStringArrayListExtra("queue_ids", ids).putStringArrayListExtra("queue_names", names).putStringArrayListExtra("queue_headings", headings))
    }

    // Updates (server/updates.rs) ----------------------------------------------------------------

    fun checkForUpdate() {
        val current = api ?: return
        io.execute { val found = Updates.available(current); post { update = found } }
    }

    fun installUpdate(activity: Activity) {
        val current = api ?: return
        val found = update ?: return
        io.execute {
            try { Updates.install(activity, current, found) }
            catch (e: Exception) { post { problem = MainActivity.message(e) } }
        }
    }
}
