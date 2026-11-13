import os
import Foundation
import Network
import SwiftUI
import Security
#if os(iOS)
import UserNotifications
#endif
#if os(iOS) || os(tvOS)
import UIKit
#endif

struct APIError: LocalizedError {
    let message: String
    var errorDescription: String? { message }
}

enum Credentials {
    static let service = "org.torrbaby.client"
    static func read(account: String = "api-token") -> String {
        let query: [String: Any] = [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: service,
                                    kSecAttrAccount as String: account, kSecReturnData as String: true]
        var result: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess, let data = result as? Data else { return "" }
        return String(data: data, encoding: .utf8) ?? ""
    }
    static func save(_ token: String, account: String = "api-token") throws {
        let query: [String: Any] = [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: service, kSecAttrAccount as String: account]
        SecItemDelete(query as CFDictionary)
        var item = query
        item[kSecValueData as String] = Data(token.utf8)
        item[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        let status = SecItemAdd(item as CFDictionary, nil)
        guard status == errSecSuccess else { throw APIError(message: "Could not save the API token to Keychain (\(status)).") }
    }
}

struct AppNotice: Identifiable {
    let id = UUID()
    let title: String
    let detail: String
    let torrentHash: String?
}
#if os(iOS)
final class NotificationPresenter: NSObject, UNUserNotificationCenterDelegate {
    static let shared = NotificationPresenter()
    func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification,
                                withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void) {
        // The in-app bottom banner already presents foreground activity.
        completionHandler([])
    }
}
#endif

@MainActor
final class ServerActivity: ObservableObject {
    static let shared = ServerActivity()
    @Published var status: DaemonStatus?
    /// Screens on show that display the status; it is fetched only while there are any.
    var watchers = 0
    /// Screens on show listing torrents, which then update every few seconds.
    var torrentWatchers = 0
}

/// For a screen listing torrents: their progress is fetched every few seconds while it is
/// on show; otherwise once a minute.
struct NeedsLiveTorrents: ViewModifier {
    @EnvironmentObject var store: Store
    func body(content: Content) -> some View {
        content
            .onAppear { ServerActivity.shared.torrentWatchers += 1; Task { await store.refresh(polling: true) } }
            .onDisappear { ServerActivity.shared.torrentWatchers = max(0, ServerActivity.shared.torrentWatchers - 1) }
    }
}

/// For a screen that shows the server's status (processing, settings, drive space):
/// the status is fetched while it is on show, and not otherwise.
struct NeedsServerStatus: ViewModifier {
    @EnvironmentObject var store: Store
    func body(content: Content) -> some View {
        content
            .onAppear { ServerActivity.shared.watchers += 1; Task { await store.refreshStatus() } }
            .onDisappear { ServerActivity.shared.watchers = max(0, ServerActivity.shared.watchers - 1) }
    }
}

@MainActor
final class Store: ObservableObject {
    @Published var profiles: [UserProfile] = []
    @Published var activeProfile: UserProfile?
    @Published var profileError: String?
    @Published var choosingProfile = false
    @Published var loadingProfiles = false
    private var switchingProfile = false
    private var resolvedProfileScope = ""
    private var profileScope: String { libraryKey + "|" + clientId }
    var profileReady: Bool { resolvedProfileScope == profileScope && activeProfile != nil }
    var profileIdentity: String { profileScope + "|" + (activeProfile?.id ?? "") }

    func resolveProfile() async throws {
        moveDataOffAddress()
        guard configured, !loadingProfiles else { return }
        if resolvedProfileScope == profileScope { return }
        loadingProfiles = true
        defer { loadingProfiles = false }
        let scope = profileScope
        activeProfile = nil
        libraryOperations = []
        libraryOperationsError = nil
        torrents = []
        library = nil
        history = []
        if let data = UserDefaults.standard.data(forKey: "viewer:" + scope),
           let saved = try? JSONDecoder().decode(UserProfile.self, from: data) {
            activeProfile = saved
            resolvedProfileScope = scope
            profileError = nil
            #if !os(tvOS)
            DownloadQueue.shared.reload()
            #endif
            Task { await self.loadProfiles() }
            return
        }
        do {
            try await registerClient()
            let state: DeviceProfileState = try await request("GET", "/v1/devices/\(clientId)/profile")
            guard scope == profileScope else { throw CancellationError() }
            profiles = state.profiles
            if let selected = state.profileId {
                guard let person = profiles.first(where: { $0.id == selected }) else {
                    throw APIError(message: "Your selected user is unavailable. Choose a user to continue.")
                }
                try rememberProfile(person)
            } else { choosingProfile = true }
            resolvedProfileScope = scope
            profileError = nil
        } catch {
            profileError = error.localizedDescription
            throw error
        }
    }

    func loadProfiles() async {
        do {
            let scope = profileScope
            let response: ProfileList = try await request("GET", "/v1/profiles")
            guard scope == profileScope else { return }
            profiles = response.profiles
            if let current = activeProfile, let fresh = profiles.first(where: { $0.id == current.id }), fresh != current { try rememberProfile(fresh) }
            profileError = nil
        } catch { profileError = error.localizedDescription }
    }

    private func rememberProfile(_ person: UserProfile) throws {
        let data = try JSONEncoder().encode(person)
        UserDefaults.standard.set(data, forKey: "viewer:" + profileScope)
        activeProfile = person
        #if !os(tvOS)
        DownloadQueue.shared.reload()
        #endif
        resolvedProfileScope = profileScope
    }

    func selectProfile(_ person: UserProfile) async throws {
        guard !switchingProfile else { throw APIError(message: "A user switch is already in progress.") }
        switchingProfile = true
        defer { switchingProfile = false }
        let scope = profileScope
        let _: DeviceProfileState = try await request("PUT", "/v1/devices/\(clientId)/profile", body: ["profile_id": person.id])
        guard scope == profileScope else { throw CancellationError() }
        // Synchronous observers stop and report under the old identity before it changes.
        NotificationCenter.default.post(name: .torrbabyProfileWillChange, object: self)
        try rememberProfile(person)
        library = nil
        history = []
        libraryError = nil
        choosingProfile = false
        profileError = nil
        await refresh()
    }

    func saveProfile(id: String?, name: String, symbol: String, textColor: String, backgroundColor: String) async throws {
        let scope = profileScope
        let person: UserProfile = try await request(id == nil ? "POST" : "PATCH", id.map { "/v1/profiles/" + $0 } ?? "/v1/profiles",
            body: ["name": name, "avatar_symbol": symbol, "text_color": textColor, "background_color": backgroundColor])
        guard scope == profileScope else { throw CancellationError() }
        if id == nil { try await selectProfile(person) }
        else {
            if activeProfile?.id == person.id { try rememberProfile(person) }
            await loadProfiles()
        }
    }

    @Published var notices: [AppNotice] = []
    @Published var library: MediaLibrary? {
        didSet {
            guard let titles = library?.titles else { titleIndex = [:]; episodeThumbnails = [:]; return }
            titleIndex = Dictionary(titles.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
            episodeThumbnails = Dictionary(titles.flatMap(\.assets).map { ($0.id, $0.thumbnail) }, uniquingKeysWith: { first, _ in first })
            ArtworkImages.warm(titles, library: libraryKey)
        }
    }
    /// Titles and episode thumbnails by id, so each artwork tile finds its revision at once
    /// instead of searching the whole library on every refresh.
    private(set) var titleIndex: [String: LibraryTitle] = [:]
    private var episodeThumbnails: [String: String?] = [:]
    func artworkRevision(id: String, variant: String) -> String? {
        switch variant {
        case "episode": return episodeThumbnails[id] ?? nil
        case "backdrop": return titleIndex[id]?.backdrop
        default: return titleIndex[id]?.poster
        }
    }
    @Published var libraryError: String?
    @Published var metadata: [MediaMetadata] = []
    @Published var transferHistory: [TransferSample] = []
    private var lastHistoryFetch = Date.distantPast
    @Published var clients: [ClientDevice] = []
    @Published var history: [PlaybackHistory] = []
    var clientId: String = {
        let existing = Credentials.read(account: "client-id")
        if UUID(uuidString: existing) != nil { return existing }
        let id = UUID().uuidString.lowercased()
        try? Credentials.save(id, account: "client-id")
        return id
    }()
    var defaultClientName: String {
        #if os(macOS)
        return Host.current().localizedName ?? ProcessInfo.processInfo.hostName
        #else
        return UIDevice.current.name
        #endif
    }
    var clientPlatform: String {
        #if os(macOS)
        return "macOS"
        #elseif os(tvOS)
        return "tvOS"
        #else
        return UIDevice.current.userInterfaceIdiom == .pad ? "iPadOS" : "iOS"
        #endif
    }
    func registerClient() async throws {
        let _: ClientDevice = try await request("POST", "/v1/clients", body: ["client_id": clientId, "name": defaultClientName, "platform": clientPlatform])
    }
    private var flushingProgress = false
    private var pendingKey: String { "pending-playback:" + libraryKey }
    func reportProgress(sessionId: String, sequence: Int, position: Double, duration: Double, ended: Bool, watchedEligible: Bool = true, reachedEnd: Bool = false) {
        guard position.isFinite, duration.isFinite, duration > 0 else { return }
        var pending = UserDefaults.standard.dictionary(forKey: pendingKey) ?? [:]
        pending[sessionId] = ["session_id": sessionId, "sequence": sequence, "position": position, "duration": duration, "ended": ended, "watched_eligible": watchedEligible, "reached_end": reachedEnd]
        UserDefaults.standard.set(pending, forKey: pendingKey)
        Task { await flushProgress() }
    }
    func flushProgress() async {
        guard !flushingProgress else { return }
        flushingProgress = true
        defer { flushingProgress = false }
        let key = pendingKey
        let pending = UserDefaults.standard.dictionary(forKey: key) ?? [:]
        for (id, value) in pending {
            guard key == pendingKey else { break }
            guard let body = value as? [String: Any] else { continue }
            do {
                try await send("POST", "/v1/playback/progress", body: body)
                var current = UserDefaults.standard.dictionary(forKey: key) ?? [:]
                if let report = current[id] as? [String: Any], report["sequence"] as? Int == body["sequence"] as? Int {
                    current.removeValue(forKey: id)
                    UserDefaults.standard.set(current, forKey: key)
                }
            } catch { break }
        }
    }
    /// There is one library, so nothing this device keeps (profile choice, library snapshot,
    /// artwork, subtitle defaults, pending playback, downloads) is keyed to the server at
    /// all. The server's name (chosen by its owner, "Diamond") is what people see, and its
    /// permanent id says it is the same server; neither is ever an address.
    static let libraryKey = "library"
    var libraryKey: String { Self.libraryKey }
    @Published private(set) var serverName = UserDefaults.standard.string(forKey: "server-name") ?? ""
    private(set) var serverID = UserDefaults.standard.string(forKey: "server-id") ?? ""
    /// Earlier builds keyed this device's data to the server's address. Moved once, at launch.
    private func moveDataOffAddress() {
        let defaults = UserDefaults.standard
        guard !defaults.bool(forKey: "data-keyed-to-library"), !server.isEmpty else { return }
        let old = server, new = Self.libraryKey, client = clientId
        func move(_ from: String, _ to: String) {
            guard from != to, let value = defaults.object(forKey: from) else { return }
            if let earlier = value as? [String: Any], let current = defaults.dictionary(forKey: to) {
                defaults.set(earlier.merging(current) { _, kept in kept }, forKey: to)
            } else if defaults.object(forKey: to) == nil { defaults.set(value, forKey: to) }
            defaults.removeObject(forKey: from)
        }
        for prefix in ["pending-playback:", "activity-watermark:", "fallback-server:"] { move(prefix + old, prefix + new) }
        move("viewer:" + old + "|" + client, "viewer:" + new + "|" + client)
        move("subtitle-defaults:" + old + ":" + client, "subtitle-defaults:" + new + ":" + client)
        for flag in ["fallback-routing-mac:", "fallback-routing-v2:"] { defaults.removeObject(forKey: flag + old) }
        #if !os(tvOS)
        DownloadQueue.shared.adoptLibrary(new)
        #endif
        defaults.set(true, forKey: "data-keyed-to-library")
    }
    @Published private(set) var homeRoute = UserDefaults.standard.string(forKey: "route-home") ?? ""
    /// Torrbaby's rendezvous on bronze ("ip:port" of its QUIC port, and its certificate's
    /// SHA-256), learned from the server's online check or the pairing code.
    private(set) var relayAddress = UserDefaults.standard.string(forKey: "relay-address") ?? ""
    private(set) var relayFingerprint = UserDefaults.standard.string(forKey: "relay-fingerprint") ?? ""
    func learnRelay(address: String?, fingerprint: String?) {
        guard let address, let fingerprint, !address.isEmpty, fingerprint.count == 64 else { return }
        relayAddress = address; relayFingerprint = fingerprint
        UserDefaults.standard.set(address, forKey: "relay-address"); UserDefaults.standard.set(fingerprint, forKey: "relay-fingerprint")
    }
    /// Torrbaby's own STUN server: the rendezvous's address on UDP 3478.
    var stunServer: String? {
        guard let host = relayAddress.split(separator: ":").first, !host.isEmpty else { return nil }
        return "\(host):3478"
    }
    @Published private(set) var awayRoute = UserDefaults.standard.string(forKey: "route-away") ?? ""
    private var routePort: Int { URLComponents(string: server)?.port ?? 8765 }
    private func address(_ host: String) -> String { host.isEmpty ? "" : "http://\(host):\(routePort)" }
    private func adopt(_ identity: ServerIdentity?) {
        guard let identity else { return }
        let defaults = UserDefaults.standard
        if let name = identity.name?.trimmingCharacters(in: .whitespacesAndNewlines), !name.isEmpty, name != serverName {
            serverName = name; defaults.set(name, forKey: "server-name")
        }
        let home = identity.routes?.home ?? "", away = identity.routes?.away ?? ""
        if !home.isEmpty && home != homeRoute { homeRoute = home; defaults.set(home, forKey: "route-home") }
        if !away.isEmpty && away != awayRoute {
            awayRoute = away; defaults.set(away, forKey: "route-away")
            fallbackServer = address(away)
        }
        if let id = identity.id?.trimmingCharacters(in: .whitespacesAndNewlines), !id.isEmpty, id != serverID {
            serverID = id; defaults.set(id, forKey: "server-id")
        }
        if let relay = identity.relay { relayServerConnected = relay.connected }
        if let relay = identity.relay, let address = relay.address, let fingerprint = relay.fingerprint,
           !address.isEmpty, fingerprint.count == 64, address != relayAddress || fingerprint != relayFingerprint {
            relayAddress = address; relayFingerprint = fingerprint
            defaults.set(address, forKey: "relay-address"); defaults.set(fingerprint, forKey: "relay-fingerprint")
        }
        // The paired address may be a number (an address from DHCP or Tailscale); once the
        // server's names are known, it is kept by name instead.
        let named = address(home)
        if !named.isEmpty && ServerRoutes.normalized(server) != named {
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                self.server = named
                UserDefaults.standard.set(named, forKey: "server")
            }
        }
    }
    @Published var server: String = UserDefaults.standard.string(forKey: "server") ?? "" {
        didSet {
            if oldValue != server {
                activeServer = nil; activeRouteTunnelled = false
                status = nil; reachable = false
                fallbackServer = savedFallback()
            }
        }
    }
    @Published var fallbackServer = "" {
        didSet {
            UserDefaults.standard.set(fallbackServer, forKey: "fallback-server:" + libraryKey)
        }
    }
    @Published private(set) var activeServer: String?
    /// The active route reached the home address through a tunnel (Tailscale
    /// subnet routing), so the device is not actually on the home network.
    @Published private(set) var activeRouteTunnelled = false
    private var connectionProbe: Task<ServerPing, Error>?
    private func savedFallback() -> String {
        if !awayRoute.isEmpty { return address(awayRoute) }
        return UserDefaults.standard.string(forKey: "fallback-server:" + libraryKey) ?? ""
    }
    /// At home: the server's Bonjour name once it is known, else the address it was paired with.
    var primaryAddress: String {
        homeRoute.isEmpty ? server : address(homeRoute)
    }
    var usingFallback: Bool {
        ServerRoutes.isFallback(primary: primaryAddress, active: activeServer, tunnelled: activeRouteTunnelled)
    }
    /// Reaching the server is all streaming needs: at home or away, it streams at full
    /// quality (away from home it may not keep up; there is no lower-quality mode).
    /// `usingFallback` only orders routes: the home name is retried while away.
    var canStream: Bool { connected }
    var connectionLabel: String { connected ? "Connected" : "Offline" }
    /// The one deliberate use of colour in the chrome: a light for the connection.
    var connectionColor: Color { connected ? .green : .red }
    func saveFallback(_ address: String) throws {
        let value = ServerRoutes.normalized(address)
        if !value.isEmpty { _ = try ServerRoutes.endpoint(value, path: "/v1/status") }
        fallbackServer = value
        activeServer = nil; activeRouteTunnelled = false
        status = nil; reachable = false
    }
    @Published var token: String = Credentials.read()
    @Published var torrents: [Torrent] = []
    /// The server's status changes on nearly every poll (uptime, free space, processing
    /// progress). It lives in `ServerActivity`, which only the screens that show it observe;
    /// the store itself announces a change only when the connection comes or goes, so the
    /// library and detail pages are not redrawn every few seconds.
    var status: DaemonStatus? {
        get { ServerActivity.shared.status }
        set {
            if (newValue == nil) != (ServerActivity.shared.status == nil) { objectWillChange.send() }
            ServerActivity.shared.status = newValue
            updateDownloadConnection()
        }
    }
    @Published var events: [AuditEvent] = []
    @Published var connectionError: String? { didSet { updateDownloadConnection() } }
    @Published var torrentError: String?
    @Published var libraryOperations: [PendingLibraryOperation] = []
    @Published var libraryOperationsError: String?
    var detachedLibraryOperations: [PendingLibraryOperation] {
        let shown = Set(torrents.compactMap { $0.fileOperation?.id })
        return libraryOperations.filter { $0.phase != "done" && !shown.contains($0.id) }
    }
    @Published var actionError: String?
    @Published var busy = false
    @Published var isConnecting = false
    @Published private(set) var networkAvailable = true { didSet { updateDownloadConnection() } }
    private func updateDownloadConnection() {
        #if !os(tvOS)
        DownloadQueue.shared.connectionChanged()
        #endif
    }
    private let networkMonitor = NWPathMonitor()
    private var refreshing = false
    private var etaSmoother = TorrentETASmoother()
    init() {
        networkMonitor.pathUpdateHandler = { [weak self] path in
            let available = path.status == .satisfied
            Task { @MainActor [weak self] in
                guard let self else { return }
                let restored = !self.networkAvailable && available
                // A restored Wi-Fi/cellular path still needs a successful server probe.
                if !available { self.connectionError = URLError(.notConnectedToInternet).localizedDescription }
                #if os(iOS)
                self.relayNetworkChanged()
                #endif
                self.networkAvailable = available
                if restored {
                    // A new network may make the primary route reachable again. Keep the
                    // route that last worked (tried first) and give the home address a quick
                    // try ahead of it; clearing the route and marking the connection lost
                    // showed Offline on every return and waited out the home address.
                    await self.refresh()
                } else if available && self.usingFallback {
                    // On cellular with Tailscale the path changes often. Keep the working
                    // fallback and only give the home address a quick try; dropping the
                    // route here showed Offline and made the status come and go.
                    self.refreshFullyNext()
                    await self.refresh(polling: true)
                }
            }
        }
        networkMonitor.start(queue: DispatchQueue(label: "org.torrbaby.connectivity"))
        #if !os(tvOS)
        DownloadQueue.shared.attach(self)
        #endif
        #if DEBUG
        // UI checks can open the user chooser directly: -initialChooser YES.
        if UserDefaults.standard.bool(forKey: "initialChooser") { choosingProfile = true }
        #endif
        #if os(iOS)
        AutoDownload.store = self
        UNUserNotificationCenter.current().delegate = NotificationPresenter.shared
        #endif
        #if os(macOS)
        if server.isEmpty, let legacy = UserDefaults.standard.persistentDomain(forName: "org.torrbaby.app"), let address = legacy["server"] as? String {
            server = address
            UserDefaults.standard.set(address, forKey: "server")
            if let compact = legacy["compactTorrentView"] as? Bool { UserDefaults.standard.set(compact, forKey: "compactTorrentView") }
        }
        #endif
        #if DEBUG
        // Optional one-use profile for local simulator/UI smoke tests. No secrets in builds.
        if let path = ProcessInfo.processInfo.environment["TORRBABY_BOOTSTRAP_FILE"],
           let data = try? Data(contentsOf: URL(fileURLWithPath: path)),
           let profile = try? JSONDecoder().decode([String: String].self, from: data),
           let address = profile["server"], let secret = profile["token"] {
            server = address
            token = secret
            // Simulators without a lasting Keychain keep one device identity across launches.
            if let id = profile["client_id"], UUID(uuidString: id) != nil { clientId = id }
            learnRelay(address: profile["relay"], fingerprint: profile["relay_fingerprint"])
            UserDefaults.standard.set(address, forKey: "server")
            try? Credentials.save(secret)
            try? FileManager.default.removeItem(atPath: path)
        }
        #endif
        fallbackServer = savedFallback()
        #if os(macOS)
        // App-owned polling continues after the last library window is closed.
        Task { [weak self] in
            while !Task.isCancelled {
                await self?.refresh(polling: true)
                let delay = self?.connectionError == nil ? 5 : 15
                try? await Task.sleep(for: .seconds(delay))
                if self == nil { break }
            }
        }
        #endif
    }
    var configured: Bool { !server.isEmpty && !token.isEmpty }
    /// The server answered the online check. Nothing else decides whether we are online.
    @Published private(set) var reachable = false
    var connected: Bool { networkAvailable && reachable && connectionError == nil }
    var offline: Bool { configured && (!networkAvailable || connectionError != nil) }
    var offlineLibrary: Bool {
        #if os(tvOS)
        return false
        #else
        return configured && !canStream
        #endif
    }
    func isDownloaded(_ asset: LibraryAsset) -> Bool {
        #if os(tvOS)
        return false
        #else
        return DownloadQueue.shared.readyDownload(assetID: asset.id, server: libraryKey) != nil
        #endif
    }
    func isDownloaded(_ title: LibraryTitle) -> Bool { title.assets.contains(where: isDownloaded) }
    /// A copy downloaded to this device still plays while the server's drive is away.
    func canPlay(_ asset: LibraryAsset) -> Bool { asset.complete && ((canStream && asset.offline != true) || isDownloaded(asset)) }
    func onMissingDrive(_ asset: LibraryAsset) -> Bool { asset.offline == true && !isDownloaded(asset) }
    func preferredLibraryAsset(_ title: LibraryTitle) -> LibraryAsset? {
        if offlineLibrary {
            if let resume = title.resume, isDownloaded(resume) { return resume }
            if let local = title.playable.first(where: isDownloaded) { return local }
        }
        let chosen = title.resume ?? title.playable.first(where: { !$0.watched }) ?? title.playable.first
        // Another copy of the same film or episode, when this one is on a drive that is away.
        if let chosen, onMissingDrive(chosen),
           let copy = title.playable.first(where: { $0.season == chosen.season && $0.episode == chosen.episode && canPlay($0) }) { return copy }
        return chosen
    }
    private let decoder: JSONDecoder = {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return decoder
    }()
    private let session: URLSession = {
        let config = URLSessionConfiguration.ephemeral
        config.timeoutIntervalForRequest = 25
        config.timeoutIntervalForResource = 45
        return URLSession(configuration: config)
    }()
    func endpoint(_ path: String) throws -> URL {
        try ServerRoutes.endpoint(directRoute ?? activeServer ?? primaryAddress, path: path)
    }
    /// Torrbaby's own direct path to the server (a local proxy into a QUIC tunnel), used
    /// instead of the Tailscale route while it is up. Only away from home, on iPhone and iPad.
    @Published private(set) var directRoute: String?
    /// The check-in's current step and since when (shown on the iPhone's test screen).
    private(set) var checkInStage = ("idle", Date.distantPast)
    func stage(_ name: String) { checkInStage = (name, Date()) }
    /// The loopback proxy into the relay through bronze, while it is open (iPhone and iPad).
    @Published private(set) var relayRoute: String?
    /// Whether Diamond said it was connected to bronze, at the last online check.
    @Published private(set) var relayServerConnected: Bool?
    /// The last answer (or failure) of each way to the server, for Settings → Connection:
    /// "home", "tailscale", "relay", "direct".
    struct RouteCheck { var ok: Bool; var ms: Int?; var detail: String; var at = Date() }
    @Published private(set) var routeChecks: [String: RouteCheck] = [:]
    func noteRoute(_ kind: String, _ check: RouteCheck) { routeChecks[kind] = check }
    private func routeKind(_ address: String) -> String {
        if address == Self.relayCandidate || (relayRoute != nil && address == relayRoute) { return "relay" }
        if directRoute != nil && address == directRoute { return "direct" }
        return ServerRoutes.normalized(address) == ServerRoutes.normalized(primaryAddress) ? "home" : "tailscale"
    }
    static func describe(_ error: Error) -> String {
        if let url = error as? URLError {
            switch url.code {
            case .cancelled: return "not needed: another way answered first"
            case .timedOut: return "no answer in time"
            case .cannotFindHost, .dnsLookupFailed: return "name not found (\(url.failingURL?.host ?? "?") does not resolve on this network)"
            case .cannotConnectToHost: return "could not connect"
            case .notConnectedToInternet: return "no network"
            case .networkConnectionLost: return "connection lost"
            default: return "network error \(url.code.rawValue)"
            }
        }
        return error.localizedDescription
    }
    /// A route's name for people: "home network", "relay via bronze", "Tailscale", "direct".
    func routeLabel(_ address: String) -> String {
        switch routeKind(address) { case "relay": "relay via bronze"; case "direct": "direct"; case "home": "home network"; default: "Tailscale" }
    }
    /// How this device reaches the server now.
    enum Path: String { case home = "Home network", direct = "Direct (hole punch)", relay = "Relay via bronze", tailscale = "Tailscale", offline = "Offline" }
    var path: Path {
        guard connected, let active = activeServer else { return .offline }
        #if os(iOS)
        if directRoute != nil { return .direct }
        if relayRoute != nil && active == relayRoute { return .relay }
        #endif
        return usingFallback ? .tailscale : .home
    }
    /// Time a request over every way to the server now (Settings → Connection).
    func measurePaths() async {
        var targets: [(String, String)] = []
        if !primaryAddress.isEmpty { targets.append(("home", primaryAddress)) }
        if !fallbackServer.isEmpty { targets.append(("tailscale", fallbackServer)) }
        #if os(iOS)
        if let relay = await ensureRelay() { targets.append(("relay", relay)) }
        else if !relayAddress.isEmpty { noteRoute("relay", RouteCheck(ok: false, ms: nil, detail: "could not open the relay to \(relayAddress)")) }
        if let direct = directRoute { targets.append(("direct", direct)) }
        #endif
        await withTaskGroup(of: Void.self) { group in
            for (kind, address) in targets {
                group.addTask { @MainActor in
                    let started = Date()
                    do {
                        let _: ServerPing = try await self.request("GET", "/v1/ping", timeout: 10, address: address)
                        self.noteRoute(kind, RouteCheck(ok: true, ms: Int(Date().timeIntervalSince(started) * 1000), detail: "answered"))
                    } catch {
                        self.noteRoute(kind, RouteCheck(ok: false, ms: nil, detail: Self.describe(error)))
                    }
                }
            }
        }
    }
    #if os(iOS)
    private var relayTunnel: RelayTunnel?
    private var relayOpening: Task<String?, Never>?
    private var lastRelayFailure = Date.distantPast
    /// The relay's loopback address, opening it when needed. Nil when the rendezvous is not
    /// known or could not be reached (not tried again for a few seconds).
    func ensureRelay() async -> String? {
        if let relayRoute, relayTunnel?.alive == true { return relayRoute }
        if let relayOpening { return await relayOpening.value }
        guard !relayAddress.isEmpty, relayFingerprint.count == 64, networkAvailable,
              Date().timeIntervalSince(lastRelayFailure) > 15 else { return nil }
        let address = relayAddress, fingerprint = relayFingerprint
        let task = Task { @MainActor () -> String? in
            guard let tunnel = await RelayTunnel.open(address: address, fingerprint: fingerprint) else {
                lastRelayFailure = Date()
                directEvent("relay: could not reach \(address)")
                return nil
            }
            relayTunnel?.close()
            tunnel.onClose = { [weak self] in self?.dropRelay("closed") }
            relayTunnel = tunnel
            let route = "http://127.0.0.1:\(tunnel.port)"
            relayRoute = route
            directEvent("relay: open on port \(tunnel.port) via \(tunnel.interface ?? "default")")
            return route
        }
        relayOpening = task
        defer { relayOpening = nil }
        return await task.value
    }
    func dropRelay(_ reason: String) {
        guard relayTunnel != nil || relayRoute != nil else { return }
        let tunnel = relayTunnel, route = relayRoute
        relayTunnel = nil; relayRoute = nil
        directEvent("relay: dropped (\(reason))")
        tunnel?.onClose = nil
        tunnel?.close()
        // Requests were going through it: find a route again now rather than at the next check.
        if let route, activeServer == route, reason != "home" {
            activeServer = nil
            refreshFullyNext()
            Task { await refresh(polling: true) }
        }
    }
    /// The relay's socket is bound to one network; a different one needs a new relay.
    private func relayNetworkChanged() {
        guard let tunnel = relayTunnel else { return }
        if tunnel.interface != PunchProbe.physicalInterface() { dropRelay("network changed") }
    }
    private var directTunnel: DirectTunnel?
    /// The last direct attempt, step by step, for Settings → Connection.
    struct DirectAttempt {
        var started = Date()
        var signal = ""
        var step = "starting"
        var interface: String?
        var publicAddress: String?
        var answered = 0, asked = 0
        var window: String?
        var server: String?
        var session: String?
        var punchMs: Int?
        var quicMs: Int?
        var finished: Date?
        var ok = false
        var failure: String?
        var serverView: String?
    }
    @Published var directAttempt: DirectAttempt?
    @Published private(set) var directOpenedAt: Date?
    /// Why there is no direct path now, or nil while one is open.
    var directWhyNot: String? {
        if directRoute != nil { return nil }
        if directAttempting { return "trying now…" }
        if !connected { return "not tried: offline (the online check has to succeed first, over the relay or Tailscale)" }
        if !usingFallback { return "not needed: on the home network" }
        if let failure = directAttempt?.failure { return failure }
        return directNote.isEmpty ? "not tried yet" : directNote
    }
    /// When the next automatic attempt is due after a failure.
    var nextDirectTry: Date? {
        guard directRoute == nil, !directAttempting, lastDirectAttempt != .distantPast else { return nil }
        let next = lastDirectAttempt.addingTimeInterval(180)
        return next > Date() ? next : nil
    }
    private var directAttempting = false
    private var lastDirectAttempt = Date.distantPast
    @Published private(set) var directNote = ""
    /// The check-in's current step and since when, and the direct path's recent events,
    /// for the test screen: which step never finishes, and why tunnels dropped.
    @Published private(set) var directEvents: [String] = []
    func directEvent(_ text: String) {
        Self.connectionLog.notice("\(text, privacy: .public)")
        directEvents.append("\(Date().formatted(date: .omitted, time: .standard)) \(text)")
        if directEvents.count > 40 { directEvents.removeFirst(directEvents.count - 40) }
    }
    /// What the direct path is doing, for Settings → About → Direct connection test.
    var directDiagnostics: String {
        let clock: (Date) -> String = { $0 == .distantPast ? "never" : $0.formatted(date: .omitted, time: .standard) }
        return "route \(activeServer ?? "none") · relay \(relayRoute ?? (relayAddress.isEmpty ? "unknown" : "closed")) · away \(usingFallback) · connected \(connected) · trying \(directAttempting) · last try \(clock(lastDirectAttempt)) · last check-in \(clock(lastFullRefresh)) · check-in at \(checkInStage.0) since \(clock(checkInStage.1))"
    }
    /// Try now, whatever the last attempt was (the test screen's button).
    private static let connectionLog = Logger(subsystem: "org.torrbaby", category: "connection")
    func tryDirectNow() { lastDirectAttempt = .distantPast; startDirectIfUseful() }
    /// Away from home, try for a direct path every few minutes until one holds.
    private func startDirectIfUseful() {
        guard connected, usingFallback, directRoute == nil, !directAttempting,
              Date().timeIntervalSince(lastDirectAttempt) > 180, let signal = activeServer else {
            let note = !connected ? "not tried: offline" : !usingFallback ? "not tried: at home" : directAttempting ? "trying…" : directNote
            if note != directNote && note.hasPrefix("not tried") { directEvent("direct \(note)") }
            directNote = note
            return
        }
        directNote = "trying…"
        directAttempting = true; lastDirectAttempt = Date()
        Task { @MainActor in
            defer { directAttempting = false }
            do {
                let tunnel = try await DirectTunnel.establish(store: self, signal: signal)
                guard usingFallback else { tunnel.close(); return }
                tunnel.onClose = { [weak self] in self?.dropDirect("closed") }
                directTunnel = tunnel
                directRoute = "http://127.0.0.1:\(tunnel.port)"
                directOpenedAt = Date()
                directEvent("open on port \(tunnel.port)")
                directNote = "open since \(Date().formatted(date: .omitted, time: .shortened)), \(tunnel.how)"
            } catch {
                directNote = "not available: \(error.localizedDescription)"
                directEvent("failed: \(error.localizedDescription)")
            }
        }
    }
    func dropDirect(_ reason: String) {
        guard directTunnel != nil || directRoute != nil else { return }
        let tunnel = directTunnel
        directTunnel = nil; directRoute = nil; directOpenedAt = nil
        directNote = reason
        directEvent("dropped: \(reason)")
        // A path that was open and dropped (the app was in the background, a brief loss) is
        // tried again at the next check; failed attempts keep their few minutes' wait.
        if reason != "home" { lastDirectAttempt = .distantPast }
        tunnel?.onClose = nil
        tunnel?.close()
    }
    #endif
    private func probeConnection() async throws -> ServerPing {
        if let connectionProbe { return try await connectionProbe.value }
        let identityServer = server, primary = primaryAddress, fallback = fallbackServer
        // Home and away are tried at the same time; home wins if it answers within a moment.
        var addresses = [primary, fallback].filter { !$0.isEmpty }
        #if os(iOS)
        // The relay through bronze joins the race unless this device is known to be at home
        // (then it would only cost a connection to bronze on every check).
        let atHome = reachable && activeServer.map(ServerRoutes.normalized) == ServerRoutes.normalized(primary) && !activeRouteTunnelled
        if !atHome && !relayAddress.isEmpty { addresses.append(Self.relayCandidate) }
        #if DEBUG
        // Simulator checks of the relay from home: -relayOnly YES leaves home and Tailscale out.
        if UserDefaults.standard.bool(forKey: "relayOnly") { addresses = [Self.relayCandidate] }
        #endif
        #endif
        let task = Task { @MainActor in
            let (_, probed): (String, (ServerPing, Bool, String)) = try await ServerRoutes.race(addresses: addresses, preferred: primary) { @MainActor candidate in
                var address = candidate
                let kind = self.routeKind(candidate)
                let started = Date()
                #if os(iOS)
                if candidate == Self.relayCandidate {
                    guard let opened = await self.ensureRelay() else {
                        self.noteRoute(kind, RouteCheck(ok: false, ms: nil, detail: "could not open the relay to \(self.relayAddress)"))
                        throw URLError(.cannotConnectToHost)
                    }
                    address = opened
                }
                #endif
                do {
                let observer = RouteObserver()
                // A tunnelled route over cellular can take longer than the LAN to answer.
                let away = ServerRoutes.normalized(address) != ServerRoutes.normalized(primary)
                let timeout: TimeInterval = away ? 10 : 4
                let value: ServerPing
                do { value = try await self.request("GET", "/v1/ping", timeout: timeout, address: address, observer: observer) }
                catch is APIError {
                    // A server from before the online check answers it with an error: use its status.
                    let full: DaemonStatus = try await self.request("GET", "/v1/status", timeout: timeout * 3, address: address, observer: observer)
                    value = ServerPing(version: full.version, server: full.server)
                }
                // iOS can carry even home Wi-Fi traffic through Tailscale when subnet routes are
                // on, so a tunnelled route only means "away" when no own interface is on the
                // subnet of the address the name led to.
                let tunnelled = observer.tunnelled && !ServerRoutes.sharesSubnet(observer.reached ?? address, interfaces: ServerRoutes.localInterfaces())
                self.noteRoute(kind, RouteCheck(ok: true, ms: Int(Date().timeIntervalSince(started) * 1000), detail: tunnelled ? "answered, through a tunnel (not actually at home)" : "answered"))
                return (value, tunnelled, address)
                } catch {
                    self.noteRoute(kind, RouteCheck(ok: false, ms: nil, detail: Self.describe(error)))
                    throw error
                }
            }
            guard self.server == identityServer, self.fallbackServer == fallback else { throw CancellationError() }
            let route = probed.2
            self.activeServer = route
            #if os(iOS)
            if ServerRoutes.normalized(route) == ServerRoutes.normalized(primary) && !probed.1 { self.dropRelay("home") }
            #endif
            self.adopt(probed.0.server)
            if !self.reachable { self.reachable = true }
            self.activeRouteTunnelled = probed.1
            return probed.0
        }
        connectionProbe = task
        defer { connectionProbe = nil }
        return try await task.value
    }
    func request<T: Decodable>(_ method: String, _ path: String, body: [String: Any]? = nil, timeout: TimeInterval? = nil, address: String? = nil, observer: RouteObserver? = nil) async throws -> T {
        var request = URLRequest(url: try address.map { try ServerRoutes.endpoint($0, path: path) } ?? endpoint(path))
        if let timeout { request.timeoutInterval = timeout }
        request.httpMethod = method
        let identity = profileIdentity
        if profileReady, let person = activeProfile { request.setValue(person.id, forHTTPHeaderField: "X-Torrbaby-Profile") }
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        if let body {
            request.httpBody = try JSONSerialization.data(withJSONObject: body)
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
        let throughDirect = address == nil && directRoute != nil
        let throughRelay = !throughDirect && relayRoute != nil && (address ?? activeServer) == relayRoute
        let (data, response): (Data, URLResponse)
        do { (data, response) = try await session.data(for: request, delegate: observer) }
        catch {
            #if os(iOS)
            // The direct path failed: back to Tailscale at once (the next try uses it).
            if throughDirect, let network = error as? URLError, network.code != .cancelled { dropDirect("failed: \(network.code.rawValue)") }
            if throughRelay, let network = error as? URLError, network.code != .cancelled, network.code != .timedOut { dropRelay("failed: \(network.code.rawValue)") }
            #endif
            throw error
        }
        guard let http = response as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            let payload = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
            throw APIError(message: payload?["error"] as? String ?? "The server could not complete the request.")
        }
        if path == "/v1/library" || path == "/v1/library/list" || path == "/v1/playback" || path.hasSuffix("/progress") {
            guard identity == profileIdentity else { throw CancellationError() }
        }
        let decoded = try decoder.decode(T.self, from: data)
        if method == "GET", path == "/v1/library" || path == "/v1/library/list" {
            await LibraryDiskCache.shared.saveLibrary(data, scope: identity)
        }
        return decoded
    }
    struct Empty: Decodable {}
    func send(_ method: String = "POST", _ path: String, body: [String: Any] = [:]) async throws {
        let _: Empty = try await request(method, path, body: body)
    }
    func connect() async {
        isConnecting = true
        defer { isConnecting = false }
        do {
            server = server.trimmingCharacters(in: .whitespacesAndNewlines).trimmingCharacters(in: CharacterSet(charactersIn: "/"))
            token = token.trimmingCharacters(in: .whitespacesAndNewlines)
            _ = try await probeConnection()
            connectionError = nil
            try Credentials.save(token)
            UserDefaults.standard.set(server, forKey: "server")
            await refresh()
        } catch { connectionError = error.localizedDescription }
    }
    @Published var subtitleQueueError: String?
    private var refreshingStatus = false
    private func refreshStatusIfShown() async { if ServerActivity.shared.watchers > 0 { await refreshStatus() } }
    func refreshStatus() async {
        guard configured, !refreshingStatus else { return }
        refreshingStatus = true
        defer { refreshingStatus = false }
        do {
            if !connected { _ = try await probeConnection(); connectionError = nil }
            status = try await request("GET", "/v1/status", timeout: 30)
            subtitleQueueError = nil
        } catch {
            guard !Self.cancelled(error) else { return }
            subtitleQueueError = "Couldn’t refresh the queue. Reconnecting…"
        }
    }
    private struct LibraryRevisions: Decodable { let revision: String; let content: String?; let progress: String? }
    /// The list and progress revisions this device holds, per profile. The list's is kept with
    /// the snapshot on disk, so waking the app downloads nothing when nothing changed.
    private var heldProgress: (identity: String, revision: String)?
    private func heldContent(_ identity: String) -> String? { UserDefaults.standard.string(forKey: "library-content-revision:" + identity) }
    private func holdContent(_ identity: String, _ revision: String?) { UserDefaults.standard.set(revision, forKey: "library-content-revision:" + identity) }
    /// Cast and tracks loaded for titles' pages, kept across list updates.
    private var titleDetails: [String: LibraryTitleDetails] = [:]
    private func withDetails(_ received: MediaLibrary) -> MediaLibrary {
        var result = received
        for (id, details) in titleDetails { result.apply(details, to: id) }
        return result
    }
    /// A title's page loads its cast and each file's tracks (the list leaves them out).
    func loadDetails(_ titleID: String) async {
        let identity = profileIdentity
        let encoded = titleID.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? titleID
        guard let details: LibraryTitleDetails = try? await request("GET", "/v1/library/titles/\(encoded)/details", timeout: 30),
              identity == profileIdentity else { return }
        titleDetails[titleID] = details
        if var current = library { current.apply(details, to: titleID); library = current }
    }
    /// The library, in parts: the list only when its revision changed (it nearly never does),
    /// then progress when that changed. A failure here is the library's own error; the server
    /// is still connected and every other page still loads.
    private func syncLibrary(_ identity: String) async {
        defer { stage("library done") }
        do {
            let revisions = try? await request("GET", "/v1/library/revision", timeout: 10) as LibraryRevisions
            guard let content = revisions?.content, let progress = revisions?.progress else {
                // An older server: the whole library each time.
                let received: MediaLibrary = try await request("GET", "/v1/library", timeout: 45)
                guard identity == profileIdentity else { return }
                library = withContinueWatchingChoices(received)
                if libraryError != nil { libraryError = nil }
                return
            }
            var progressHeld = heldProgress?.identity == identity ? heldProgress?.revision : nil
            if library == nil || heldContent(identity) != content {
                // Several hundred kilobytes; a slow route gets time to finish.
                let received: MediaLibrary = try await request("GET", "/v1/library/list", timeout: 45)
                guard identity == profileIdentity else { return }
                library = withContinueWatchingChoices(withDetails(received))
                holdContent(identity, content)
                progressHeld = nil
            }
            if progressHeld != progress {
                let received: LibraryProgress = try await request("GET", "/v1/library/progress", timeout: 20)
                guard identity == profileIdentity, var current = library else { return }
                current.apply(received)
                library = withContinueWatchingChoices(current)
                heldProgress = (identity, received.revision)
            }
            if libraryError != nil { libraryError = nil }
        } catch {
            guard !Self.cancelled(error), identity == profileIdentity else { return }
            libraryError = error.localizedDescription
        }
    }
    private func syncTorrentsAndOperations(_ identity: String) async {
        async let torrents: Void = syncTorrents(identity)
        async let operations: Void = syncOperations(identity)
        _ = await (torrents, operations)
    }
    private func syncTorrents(_ identity: String) async {
        do {
            let received: [Torrent] = try await request("GET", "/v1/torrents")
            guard identity == profileIdentity else { return }
            let next = etaSmoother.update(received, server: server, now: Date().timeIntervalSince1970)
                .sorted { $0.addedDate > $1.addedDate }
            if next != torrents { torrents = next }
            if torrentError != nil { torrentError = nil }
        } catch {
            guard !Self.cancelled(error), identity == profileIdentity else { return }
            torrentError = error.localizedDescription
        }
    }
    private func syncOperations(_ identity: String) async {
        do {
            let received: PendingLibraryOperations = try await request("GET", "/v1/library/operations")
            guard identity == profileIdentity else { return }
            if !(received.operations.isEmpty && libraryOperations.isEmpty) { libraryOperations = received.operations }
            if libraryOperationsError != nil { libraryOperationsError = nil }
        } catch {
            guard !Self.cancelled(error), identity == profileIdentity else { return }
            libraryOperationsError = error.localizedDescription
        }
    }
    /// Activity, devices, history and rates; each keeps what it had if it cannot load now.
    private func syncActivity(_ identity: String) async {
        defer { stage("activity done") }
        if let newEvents: [AuditEvent] = try? await request("GET", "/v1/events"), identity == profileIdentity {
            receiveActivity(newEvents)
            publish(\.events, Array(newEvents.reversed()))
        }
        try? await registerClient()
        if let received: [ClientDevice] = try? await request("GET", "/v1/clients") { publish(\.clients, received) }
        await flushProgress()
        #if !os(tvOS)
        await DownloadQueue.shared.syncProgress()
        #if os(iOS)
        // Alongside, not inside: queuing a download waits for the server to prepare the file,
        // minutes over a slow route, and the check-in never finished meanwhile.
        Task { @MainActor in await AutoDownload.run(store: self) }
        #endif
        #endif
        if let received: [PlaybackHistory] = try? await request("GET", "/v1/playback"), identity == profileIdentity { publish(\.history, received) }
        if Date().timeIntervalSince(lastHistoryFetch) >= 30 {
            lastHistoryFetch = Date()
            // Rates and metadata (over a megabyte) load in the background: the check-in does
            // not wait for them, so a slow route cannot hold it up.
            Task { @MainActor in
                if let rates: [TransferSample] = try? await request("GET", "/v1/rates") { transferHistory = rates }
                if let received: [MediaMetadata] = try? await request("GET", "/v1/metadata") { publish(\.metadata, received) }
            }
        }
        #if !os(tvOS)
        DownloadQueue.shared.importSharedProgress()
        #endif
    }
    /// When the last full refresh (online check, library, activity) finished. The periodic
    /// refresh runs every few seconds but does that only once a minute; in between it only
    /// updates what is on screen and live (torrents on the Downloads page, the status on
    /// the processing and settings screens).
    private var lastFullRefresh = Date.distantPast
    static let fullRefreshInterval: TimeInterval = 60
    /// Stands for the relay in the online check's race until it is opened.
    static let relayCandidate = "relay://bronze"
    /// The next periodic refresh does everything (the app came to the foreground), and away
    /// from home without a direct path one is tried at once.
    func refreshFullyNext() {
        lastFullRefresh = .distantPast
        #if os(iOS)
        if directRoute == nil { lastDirectAttempt = .distantPast }
        #endif
    }
    /// `polling` marks the periodic refresh; a refresh after a user action does everything
    /// and waits for one already under way so it shows that action.
    func refresh(polling: Bool = false) async {
        // A refresh after a user action must show that action: wait out a poll that
        // started earlier rather than being dropped in its favour.
        while !polling && refreshing && !Task.isCancelled { try? await Task.sleep(for: .milliseconds(50)) }
        guard configured, !refreshing else { return }
        refreshing = true
        defer { refreshing = false }
        do {
            stage("profile")
            try await resolveProfile()
            guard profileReady else { stage("no profile"); return }
            let identity = profileIdentity
            if library == nil, let saved = await LibraryDiskCache.shared.library(scope: identity) {
                guard identity == profileIdentity else { return }
                library = saved
            }
            // Restore browsing before contacting the server. A satisfied network path
            // does not mean the home server is reachable (for example while travelling).
            guard networkAvailable else { throw URLError(.notConnectedToInternet) }
            let full = !polling || !connected || Date().timeIntervalSince(lastFullRefresh) >= Self.fullRefreshInterval
            if full {
                // Connected means the server answers. Everything else loads on its own, at
                // the same time, and a page that cannot load shows its own error.
                stage("online check")
                _ = try await probeConnection()
                stage("loading")
                #if os(iOS)
                // Away from home the direct path is the default: open it at once, alongside.
                if usingFallback { startDirectIfUseful() } else { dropDirect("home") }
                #endif
                guard identity == profileIdentity else { return }
                if connectionError != nil { connectionError = nil }
                if subtitleQueueError != nil { subtitleQueueError = nil }
                async let libraryPart: Void = syncLibrary(identity)
                async let torrentsPart: Void = syncTorrents(identity)
                async let operationsPart: Void = syncOperations(identity)
                async let activityPart: Void = syncActivity(identity)
                async let statusPart: Void = refreshStatusIfShown()
                _ = await (libraryPart, torrentsPart, operationsPart, activityPart, statusPart)
                lastFullRefresh = Date()
                stage("done")
            } else {
                async let torrentsPart: Void = ServerActivity.shared.torrentWatchers > 0 ? syncTorrentsAndOperations(identity) : ()
                async let statusPart: Void = refreshStatusIfShown()
                _ = await (torrentsPart, statusPart)
            }
        } catch {
            // A refresh cut short (the app went to the background, the view changed) says
            // nothing about the server; showing it read "cancelled" and flashed Offline.
            if !Self.cancelled(error) { connectionError = error.localizedDescription }
        }
    }
    /// Every view that reads the store redraws when any published value is assigned, so the
    /// poll every few seconds assigns only what actually changed. Assigning unchanged status,
    /// devices, history and events each time redrew every visible poster and episode row,
    /// which showed as stutter while scrolling.
    private func publish<T: Encodable>(_ key: ReferenceWritableKeyPath<Store, T>, _ value: T) {
        let encoder = JSONEncoder()
        encoder.outputFormatting = .sortedKeys
        if let current = try? encoder.encode(self[keyPath: key]), let next = try? encoder.encode(value), current == next { return }
        self[keyPath: key] = value
    }
    static func cancelled(_ error: Error) -> Bool {
        error is CancellationError || (error as? URLError)?.code == .cancelled
    }
    @Published var continueWatchingUndo: (id: String, profile: String)?
    /// Choices made on this device that the server is still being told about. They are
    /// applied to any library that arrives meanwhile, so the shelf changes at once and a
    /// response fetched before the choice cannot put the title back.
    private var continueWatchingChoices: [String: Bool] = [:]
    private func withContinueWatchingChoices(_ received: MediaLibrary) -> MediaLibrary {
        guard !continueWatchingChoices.isEmpty else { return received }
        var result = received
        for index in result.titles.indices {
            if let hidden = continueWatchingChoices[result.titles[index].id] { result.titles[index].continueWatchingHidden = hidden }
        }
        return result
    }
    func setContinueWatchingHidden(_ id: String, hidden: Bool) async {
        let identity=profileIdentity
        let previous = library?.titles.first { $0.id == id }?.continueWatchingHidden
        continueWatchingChoices[id] = hidden
        if let current = library { library = withContinueWatchingChoices(current) }
        do {
            let _: Empty = try await request("POST", "/v1/library/continue-watching", body: ["title_id":id,"hidden":hidden])
            guard identity == profileIdentity else { continueWatchingChoices[id] = nil; return }
            continueWatchingUndo = hidden ? (id,identity) : nil
            await refresh()
            continueWatchingChoices[id] = nil
        } catch {
            continueWatchingChoices[id] = nil
            if identity == profileIdentity, let index = library?.titles.firstIndex(where: { $0.id == id }) { library?.titles[index].continueWatchingHidden = previous }
            actionError = error.localizedDescription
        }
    }
    func notify(_ title: String, detail: String = "", torrentHash: String? = nil) {
        notices.append(AppNotice(title: title, detail: detail, torrentHash: torrentHash))
        if notices.count > 8 { notices.removeFirst(notices.count - 8) }
        #if os(iOS)
        Task {
            let center = UNUserNotificationCenter.current()
            let settings = await center.notificationSettings()
            guard settings.authorizationStatus == .authorized || settings.authorizationStatus == .provisional else { return }
            let content = UNMutableNotificationContent()
            content.title = title
            content.body = detail
            content.sound = .default
            try? await center.add(UNNotificationRequest(identifier: UUID().uuidString, content: content, trigger: nil))
        }
        #endif
    }
    private func receiveActivity(_ incoming: [AuditEvent]) {
        let key = "activity-watermark:" + libraryKey
        let last = UserDefaults.standard.object(forKey: key) as? Double
        let newest = incoming.map(\.time).max() ?? Date().timeIntervalSince1970
        UserDefaults.standard.set(max(last ?? 0, newest), forKey: key)
        guard let last, UserDefaults.standard.object(forKey: "activityAlerts") as? Bool ?? true else { return }
        let recent = incoming.filter { $0.time > last && $0.time > Date().timeIntervalSince1970 - 86400 && ["download_complete", "seeding_ratio_reached", "trial_complete", "trial_needs_attention"].contains($0.kind) }.sorted { $0.time < $1.time }
        if recent.count > 3 {
            notify("Torrent activity", detail: "\(recent.count) downloads or seeding goals completed.")
        } else {
            for event in recent {
                notify(["download_complete":"Download complete","seeding_ratio_reached":"Seeding goal reached","trial_complete":"Trial complete","trial_needs_attention":"Trial needs attention"][event.kind] ?? "Torrent activity", detail: event.kind == "trial_needs_attention" ? L10n.text(event.name ?? "") : event.name ?? "", torrentHash: event.hash)
            }
        }
    }
    func perform(_ operation: @escaping () async throws -> Void) {
        guard !busy else { return }
        busy = true
        Task {
            defer { busy = false }
            do { try await operation(); lastHistoryFetch = .distantPast; await refresh() }
            catch { actionError = error.localizedDescription }
        }
    }
    func action(_ torrent: Torrent, _ action: String, body: [String: Any] = [:]) {
        guard torrent.fileOperation == nil else {
            actionError = "The server is managing this package’s removal. Its status is shown in Seeding."
            return
        }
        perform { try await self.send("POST", "/v1/torrents/\(torrent.id)/\(action)", body: body) }
    }
    func toggleAutomation() {
        guard let status else { return }
        perform { try await self.send("PATCH", "/v1/settings", body: ["paused": !status.settings.paused]) }
    }
}

extension Store {
    func requireStreamingConnection() throws {
        guard canStream else {
            throw APIError(message: Self.unreachable)
        }
    }
    /// Why a title cannot play while the server is out of reach. An Apple TV keeps no
    /// downloads, so there the only thing to say is that the library cannot be reached,
    /// which is usually over in a moment (the server restarting).
    static var unreachable: String {
        #if os(tvOS)
        "Can’t reach your library right now. Try again in a moment."
        #else
        "This title isn’t downloaded. Reconnect to your library to play it."
        #endif
    }
    /// Request native virtual MP4 tracks without copying the audiovisual stream.
    func mediaPlaybackLink(asset: LibraryAsset, titleID: String, startSession: Bool = true, progress: @escaping (String) -> Void = { _ in }) async throws -> PlaybackLink {
        if startSession { try requireStreamingConnection() }
        else if !connected { throw URLError(.notConnectedToInternet) }
        // The player reads the file's tracks (subtitles in the picture, picture shape); the
        // list leaves them out, so a title started without opening its page loads them now.
        if startSession, titleIndex[titleID]?.assets.first(where: { $0.id == asset.id })?.mediaInfo == nil { await loadDetails(titleID) }
        let serverAtStart = server
        let profileAtStart = profileIdentity
        let base = "/v1/library/assets/\(asset.id)"
        var body: [String: Any] = startSession ? ["client_id": clientId] : [:]
        var ids: [String] = []
        // Localized short track names ("English Voice Transcribed") by job ID.
        var names: [String: String] = [:]
        progress("Preparing playback…")
        struct Track: Decodable {
            let track: Int; let codec: String; let language: String?; let extractionSupported: Bool
        }
        struct Inventory: Decodable { let embeddedTracks: [Track] }
        struct Job: Decodable { let id: String; let state: String; let error: String? }
        func wait(_ job: Job, route: String) async throws -> Job {
            var result = job
            let deadline = Date().addingTimeInterval(1200)
            while result.state == "queued" || result.state == "running" || result.state == "preparing" {
                try Task.checkCancellation()
                if startSession { try self.requireStreamingConnection() }
                guard self.server == serverAtStart && self.profileIdentity == profileAtStart else { throw CancellationError() }
                guard Date() < deadline else { throw APIError(message: "Playback is still preparing. Try again shortly.") }
                try await Task.sleep(for: .milliseconds(500))
                result = try await self.request("GET", route + result.id)
            }
            guard result.state == "ready" else { throw APIError(message: result.error ?? "Playback couldn’t be prepared.") }
            return result
        }
        do {
            // Ready sidecar and translated jobs do not require embedded captions.
            // Include every ready language even when the device starts with subtitles Off.
            let ready: MediaTrackInfo = try await request("GET", base + "/media-info")
            ids = Array(Set(ready.prepared.filter { $0.state == "ready" }.map(\.id))).sorted()
            for job in ready.prepared where job.state == "ready" {
                names[job.id] = SubtitleTrackName.text(language: job.language, kind: job.kind, sdh: job.sdh == true)
            }
            if ids.isEmpty {
                let inventory: Inventory = try await request("GET", base + "/subtitles")
                if let source = inventory.embeddedTracks.first(where: { $0.extractionSupported }) {
                    var originalBody: [String: Any] = ["track": source.track]
                    let language = (source.language ?? "").lowercased()
                    if language == "eng" || language.hasPrefix("en") { originalBody["source"] = "en" }
                    else if language == "zho" || language == "chi" || language.hasPrefix("zh") { originalBody["source"] = "zh-Hans" }
                    let original: Job = try await request("POST", base + "/subtitles/prepare", body: originalBody)
                    ids.append(try await wait(original, route: "/v1/subtitle-jobs/").id)
                }
            }
        } catch {
            try Task.checkCancellation()
            guard server == serverAtStart && profileIdentity == profileAtStart else { throw CancellationError() }
            notify("Subtitles unavailable", detail: error.localizedDescription)
        }
        if startSession { try requireStreamingConnection() }
        let prepared: Job = try await request("POST", base + "/virtual-mp4", body: ["subtitle_jobs": ids, "subtitle_names": ids.map { names[$0] ?? "" }])
        if prepared.state != "direct" {
            body["virtual_job"] = try await wait(prepared, route: "/v1/virtual-jobs/").id
        }
        try Task.checkCancellation()
        guard server == serverAtStart && profileIdentity == profileAtStart else { throw CancellationError() }
        if startSession { try requireStreamingConnection() }
        return try await request("POST", base + "/play", body: body)
    }
}

extension Store {
    var deviceSubtitlePath: String { "/v1/devices/\(clientId)/subtitle-preferences" }
}

struct UserProfile: Codable, Identifiable, Equatable {
    let id: String
    let name: String
    var avatarSymbol: String? = nil
    var textColor: String? = nil
    var backgroundColor: String? = nil
    /// The avatar's one character: the chosen symbol's first (an emoji counts as one), else the name's initial.
    var symbol: String {
        let chosen = (avatarSymbol ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        return chosen.isEmpty ? String(name.trimmingCharacters(in: .whitespacesAndNewlines).prefix(1)).uppercased() : String(chosen.prefix(1))
    }
}
private struct ProfileList: Decodable { let profiles: [UserProfile] }
private struct DeviceProfileState: Decodable {
    let profileId: String?
    let profiles: [UserProfile]
}
extension Notification.Name {
    static let torrbabyProfileWillChange = Notification.Name("org.torrbaby.profile-will-change")
}

struct ProfileGate<Content: View>: View {
    @Environment(\.scenePhase) private var scenePhase
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    @ViewBuilder var content: Content
    var body: some View {
        Group {
            if store.configured && (!store.profileReady || store.choosingProfile) {
                ProfilePicker()
                    #if !os(tvOS)
                    // Leaves the language switch its own line above the heading.
                    .safeAreaInset(edge: .top, spacing: 0) { Color.clear.frame(height: 48) }
                    #endif
            } else {
                content.id(store.profileIdentity)
            }
        }
        .overlay(alignment: .topTrailing) {
            if store.configured && (!store.profileReady || store.choosingProfile) {
                #if os(tvOS)
                // A strip across the top for the remote, so left, right and down all leave the switch sensibly.
                HStack { Spacer(); InterfaceLanguagePicker() }.padding(16).frame(maxWidth: .infinity).focusSection()
                #else
                InterfaceLanguagePicker().padding(16)
                #endif
            }
        }
        .onChange(of: scenePhase) { _, phase in
            if phase == .active && store.profileReady { Task { await store.loadProfiles() } }
        }
        .task(id: store.server + "|" + store.clientId) {
            guard store.configured else { return }
            do { try await store.resolveProfile() } catch { }
        }
    }
}

struct ProfileAvatar: View {
    let profile: UserProfile
    var size: CGFloat = 54
    var body: some View {
        Text(verbatim: profile.symbol).font(.system(size: size * 0.48, weight: .semibold))
            .foregroundStyle(profileColor(profile.textColor ?? "#FFFFFF"))
            .frame(width: size, height: size)
            .background(profileColor(profile.backgroundColor ?? "#2A2A2A"), in: Circle())
            .accessibilityHidden(true)
    }
}
func profileColor(_ hex: String) -> Color {
    let value = UInt64(hex.trimmingCharacters(in: CharacterSet(charactersIn: "#")), radix: 16) ?? 0x243A38
    return Color(red: Double((value >> 16) & 255) / 255, green: Double((value >> 8) & 255) / 255, blue: Double(value & 255) / 255)
}

struct ProfilePicker: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    @State private var editing: UserProfile?
    @State private var creating = false
    @State private var working = false
    @FocusState private var focusedProfile: String?
    /// Wide, centred bars on the television; a readable column elsewhere.
    private var columnWidth: CGFloat {
        #if os(tvOS)
        1180
        #else
        760
        #endif
    }
    var body: some View {
        ScrollView {
            VStack(alignment: .center, spacing: 22) {
                Text("Who's watching?").font(.largeTitle.bold()).frame(maxWidth: .infinity, alignment: .center)
                if store.loadingProfiles || working { ProgressView() }
                if let error = store.profileError {
                    // Said plainly, with what to check and one way forward.
                    HStack(alignment: .center, spacing: 18) {
                        Image(systemName: store.connected ? "exclamationmark.triangle" : "wifi.exclamationmark").font(.title2).foregroundStyle(.orange).accessibilityHidden(true)
                        VStack(alignment: .leading, spacing: 4) {
                            Text(store.connected ? LocalizedStringKey(L10n.text(error)) : "Can’t reach your server").font(.headline)
                            if !store.connected { Text("Check that the server is on and that this device is on the same network.").font(.subheadline).foregroundStyle(Theme.muted) }
                        }
                        Spacer(minLength: 12)
                        Button("Try again") { Task { await store.loadProfiles() } }.buttonStyle(MonoButtonStyle(compact: true)).fixedSize()
                    }.padding(18).frame(maxWidth: .infinity).background(Theme.panel, in: RoundedRectangle(cornerRadius: 12))
                }
                ForEach(store.profiles) { person in
                    HStack(spacing: 16) {
                        Button {
                            working = true
                            Task {
                                defer { working = false }
                                do { try await store.selectProfile(person) }
                                catch { store.profileError = error.localizedDescription }
                            }
                        } label: {
                            HStack(spacing: 20) {
                                ProfileAvatar(profile: person, size: 72)
                                Text(verbatim: person.name).font(.title2.weight(.semibold))
                                Spacer()
                                // Takes the row's own colour, so it stays visible when the row is focused and turns white.
                                if person.id == store.activeProfile?.id { Image(systemName: "checkmark.circle.fill") }
                            }.padding(16).contentShape(RoundedRectangle(cornerRadius: 8))
                        }.buttonStyle(MonoButtonStyle()).focused($focusedProfile, equals: person.id)
                        Button { editing = person } label: { Image(systemName: "pencil").frame(width: 44, height: 44) }
                            .buttonStyle(MonoButtonStyle()).accessibilityLabel("Edit profile")
                    }
                }
                // As wide as the users above it, and in the same style. There is no "back" button:
                // choosing the user already in use returns to the library, as does the remote's Back.
                Button { creating = true } label: {
                    Label("Create user", systemImage: "plus").font(.title3.weight(.semibold)).frame(maxWidth: .infinity).padding(.vertical, 10)
                }.buttonStyle(MonoButtonStyle())
            }.disabled(working).padding(40).frame(maxWidth: columnWidth).frame(maxWidth: .infinity)
                #if os(tvOS)
                // The whole width belongs to this column for the remote: moving down from the
                // language switch in the corner lands here, though nothing sits right below it.
                .focusSection()
                // The remote starts on the user already in use, not on Create user.
                .defaultFocus($focusedProfile, store.activeProfile?.id ?? store.profiles.first?.id, priority: .userInitiated)
                #endif
        }.background(Theme.canvas)
            #if os(tvOS)
            .onExitCommand { if store.profileReady { store.choosingProfile = false } }
            #endif
            .task {
                await store.loadProfiles()
                #if os(tvOS)
                // Once the users are on screen, put the remote on the one in use.
                try? await Task.sleep(for: .milliseconds(350))
                focusedProfile = store.activeProfile?.id ?? store.profiles.first?.id
                #endif
            }
            .sheet(isPresented: $creating) { LocalizedInterface { ProfileEditor(profile: nil) } }
            .sheet(item: $editing) { person in LocalizedInterface { ProfileEditor(profile: person) } }
    }
}

struct ProfileEditor: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) private var dismiss
    let profile: UserProfile?
    @State private var name: String
    @State private var symbol: String
    @State private var foreground: String
    @State private var background: String
    @State private var saving = false
    @State private var error: String?
    init(profile: UserProfile?) {
        self.profile = profile
        _name = State(initialValue: profile?.name ?? "")
        _symbol = State(initialValue: profile?.avatarSymbol ?? "")
        _foreground = State(initialValue: profile?.textColor ?? "#FFFFFF")
        _background = State(initialValue: profile?.backgroundColor ?? "#2A2A2A")
    }
    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 22) {
                    ProfileAvatar(profile: UserProfile(id: "preview", name: name, avatarSymbol: symbol, textColor: foreground, backgroundColor: background), size: 100)
                        .frame(maxWidth: .infinity)
                    TextField("First name", text: $name).profileFieldStyle()
                    // Not cut short while typing: a pinyin or other input method composes several
                    // letters before it commits a character. Only the first character is kept on save.
                    TextField("Initial or emoji", text: $symbol).profileFieldStyle()
                    ProfileColorPicker(title: "Text color", hex: $foreground)
                    ProfileColorPicker(title: "Background color", hex: $background)
                    if let error { Text(L10n.text(error)).foregroundStyle(.orange) }
                    if saving { ProgressView() }
                }.padding(28)
            }.navigationTitle(LocalizedStringKey(profile == nil ? "Create user" : "Edit profile"))
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                    ToolbarItem(placement: .confirmationAction) {
                        Button("Save profile") {
                            saving = true
                            Task {
                                defer { saving = false }
                                do {
                                    let chosen = String(symbol.trimmingCharacters(in: .whitespacesAndNewlines).prefix(1))
                                    try await store.saveProfile(id: profile?.id, name: name.trimmingCharacters(in: .whitespacesAndNewlines), symbol: chosen, textColor: foreground, backgroundColor: background)
                                    dismiss()
                                } catch { self.error = error.localizedDescription }
                            }
                        }.disabled(saving || name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    }
                }
        }
        #if os(macOS)
        .frame(width: 580, height: 650)
        #endif
    }
}

struct ProfileColorPicker: View {
    let title: String
    @Binding var hex: String
    private let colors = ["#FFFFFF", "#FFFFFF", "#F5C974", "#F18FA8", "#A89CF5", "#7DC6FF", "#2A2A2A", "#18202B", "#4C2452", "#6A3434"]
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(LocalizedStringKey(title)).font(.headline)
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 44))], spacing: 12) {
                ForEach(colors, id: \.self) { value in
                    Button { hex = value } label: {
                        Circle().fill(profileColor(value)).frame(width: 36, height: 36)
                            .overlay { Circle().stroke(hex == value ? Theme.accent : .gray.opacity(0.4), lineWidth: hex == value ? 4 : 1) }
                    }.buttonStyle(.plain).accessibilityLabel(value)
                }
            }
            TextField("#RRGGBB", text: $hex).profileFieldStyle()
            #if !os(tvOS)
            ColorPicker(LocalizedStringKey(title), selection: Binding(get: { profileColor(hex) }, set: { color in
                #if os(macOS)
                guard let rgb = NSColor(color).usingColorSpace(.deviceRGB) else { return }
                let red = rgb.redComponent, green = rgb.greenComponent, blue = rgb.blueComponent
                #else
                var red: CGFloat = 0, green: CGFloat = 0, blue: CGFloat = 0, alpha: CGFloat = 0
                UIColor(color).getRed(&red, green: &green, blue: &blue, alpha: &alpha)
                #endif
                hex = String(format: "#%02X%02X%02X", Int((red * 255).rounded()), Int((green * 255).rounded()), Int((blue * 255).rounded()))
            }), supportsOpacity: false)
            #endif
        }
    }
}

private extension View {
    @ViewBuilder func profileFieldStyle() -> some View {
        #if os(tvOS)
        self
        #else
        self.textFieldStyle(.roundedBorder)
        #endif
    }
}

/// Records which of this device's addresses a request left from.
final class RouteObserver: NSObject, URLSessionTaskDelegate, @unchecked Sendable {
    private let lock = NSLock()
    private var local: String?
    private var remote: String?
    func urlSession(_ session: URLSession, task: URLSessionTask, didFinishCollecting metrics: URLSessionTaskMetrics) {
        let transaction = metrics.transactionMetrics.last { $0.localAddress != nil }
        lock.lock(); local = transaction?.localAddress; remote = transaction?.remoteAddress; lock.unlock()
    }
    /// The address the route's name led to this time, as a URL for subnet checks.
    var reached: String? {
        lock.lock(); defer { lock.unlock() }
        return remote.map { "http://" + ($0.contains(":") ? "[\($0)]" : $0) }
    }
    /// Left from a Tailscale address: the route goes through the tunnel.
    var tunnelled: Bool {
        lock.lock(); defer { lock.unlock() }
        return local.map(ServerRoutes.isTailscaleIP) ?? false
    }
}
