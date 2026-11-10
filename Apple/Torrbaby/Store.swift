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
final class Store: ObservableObject {
    @Published var profiles: [UserProfile] = []
    @Published var activeProfile: UserProfile?
    @Published var profileError: String?
    @Published var choosingProfile = false
    @Published var loadingProfiles = false
    private var switchingProfile = false
    private var resolvedProfileScope = ""
    private var profileScope: String { server + "|" + clientId }
    var profileReady: Bool { resolvedProfileScope == profileScope && activeProfile != nil }
    var profileIdentity: String { profileScope + "|" + (activeProfile?.id ?? "") }

    func resolveProfile() async throws {
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
    @Published var library: MediaLibrary?
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
    private var pendingKey: String { "pending-playback:" + server }
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
    @Published var server: String = UserDefaults.standard.string(forKey: "server") ?? ""
    @Published var token: String = Credentials.read()
    @Published var torrents: [Torrent] = []
    @Published var status: DaemonStatus?
    @Published var events: [AuditEvent] = []
    @Published var connectionError: String?
    @Published var torrentError: String?
    @Published var libraryOperations: [PendingLibraryOperation] = []
    @Published var libraryOperationsError: String?
    var detachedLibraryOperations: [PendingLibraryOperation] {
        let shown = Set(torrents.compactMap { $0.fileOperation?.id })
        return libraryOperations.filter { $0.phase != "done" && !shown.contains($0.id) }
    }
    @Published var actionError: String?
    @Published var busy = false
    @Published var lastUpdated: Date?
    @Published var isConnecting = false
    @Published private(set) var networkAvailable = true
    private let networkMonitor = NWPathMonitor()
    private var refreshing = false
    private var etaSmoother = TorrentETASmoother()
    init() {
        networkMonitor.pathUpdateHandler = { [weak self] path in
            let available = path.status == .satisfied
            Task { @MainActor [weak self] in
                guard let self else { return }
                let restored = !self.networkAvailable && available
                self.networkAvailable = available
                if restored { await self.refresh() }
            }
        }
        networkMonitor.start(queue: DispatchQueue(label: "org.torrbaby.connectivity"))
        #if !os(tvOS)
        DownloadQueue.shared.attach(self)
        #endif
        #if os(iOS)
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
            UserDefaults.standard.set(address, forKey: "server")
            try? Credentials.save(secret)
            try? FileManager.default.removeItem(atPath: path)
        }
        #endif
        #if os(macOS)
        // App-owned polling continues after the last library window is closed.
        Task { [weak self] in
            while !Task.isCancelled {
                await self?.refresh()
                let delay = self?.connectionError == nil ? 5 : 15
                try? await Task.sleep(for: .seconds(delay))
                if self == nil { break }
            }
        }
        #endif
    }
    var configured: Bool { !server.isEmpty && !token.isEmpty }
    var connected: Bool { networkAvailable && status != nil && connectionError == nil }
    var offline: Bool { configured && (!networkAvailable || connectionError != nil) }
    var offlineLibrary: Bool {
        #if os(tvOS)
        return false
        #else
        return offline
        #endif
    }
    func isDownloaded(_ asset: LibraryAsset) -> Bool {
        #if os(tvOS)
        return false
        #else
        return DownloadQueue.shared.readyDownload(assetID: asset.id, server: server) != nil
        #endif
    }
    func isDownloaded(_ title: LibraryTitle) -> Bool { title.assets.contains(where: isDownloaded) }
    func canPlay(_ asset: LibraryAsset) -> Bool { asset.complete && (!offlineLibrary || isDownloaded(asset)) }
    func preferredLibraryAsset(_ title: LibraryTitle) -> LibraryAsset? {
        if offlineLibrary {
            if let resume = title.resume, isDownloaded(resume) { return resume }
            if let local = title.playable.first(where: isDownloaded) { return local }
        }
        return title.resume ?? title.playable.first(where: { !$0.watched }) ?? title.playable.first
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
        guard let base = URL(string: server.trimmingCharacters(in: .whitespacesAndNewlines)),
              ["http", "https"].contains(base.scheme?.lowercased() ?? ""), base.host != nil,
              base.user == nil, base.password == nil, base.query == nil, base.fragment == nil,
              let url = URL(string: server.trimmingCharacters(in: CharacterSet(charactersIn: "/")) + path)
        else { throw APIError(message: "Enter a server URL such as http://your-server:8765.") }
        return url
    }
    func request<T: Decodable>(_ method: String, _ path: String, body: [String: Any]? = nil, timeout: TimeInterval? = nil) async throws -> T {
        var request = URLRequest(url: try endpoint(path))
        if let timeout { request.timeoutInterval = timeout }
        request.httpMethod = method
        let identity = profileIdentity
        if profileReady, let person = activeProfile { request.setValue(person.id, forHTTPHeaderField: "X-Torrbaby-Profile") }
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        if let body {
            request.httpBody = try JSONSerialization.data(withJSONObject: body)
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            let payload = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
            throw APIError(message: payload?["error"] as? String ?? "The server could not complete the request.")
        }
        if path == "/v1/library" || path == "/v1/playback" || path.hasSuffix("/progress") {
            guard identity == profileIdentity else { throw CancellationError() }
        }
        let decoded = try decoder.decode(T.self, from: data)
        if method == "GET", path == "/v1/library" {
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
            let _: DaemonStatus = try await request("GET", "/v1/status")
            try Credentials.save(token)
            UserDefaults.standard.set(server, forKey: "server")
            await refresh()
        } catch { connectionError = error.localizedDescription }
    }
    @Published var subtitleQueueError: String?
    private var refreshingStatus = false
    func refreshStatus() async {
        guard configured, !refreshingStatus else { return }
        refreshingStatus = true
        defer { refreshingStatus = false }
        do {
            let value: DaemonStatus = try await request("GET", "/v1/status", timeout: 8)
            status = value
            subtitleQueueError = nil
        } catch {
            subtitleQueueError = "Couldn’t refresh the queue. Reconnecting…"
        }
    }
    func refresh() async {
        guard configured, !refreshing else { return }
        refreshing = true
        defer { refreshing = false }
        do {
            try await resolveProfile()
            guard profileReady else { return }
            let identity = profileIdentity
            if library == nil, let saved = await LibraryDiskCache.shared.library(scope: identity) {
                guard identity == profileIdentity else { return }
                library = saved
            }
            do {
                let receivedLibrary: MediaLibrary = try await request("GET", "/v1/library")
                guard identity == profileIdentity else { return }
                library = receivedLibrary
                libraryError = nil
            } catch { libraryError = error.localizedDescription }
            await refreshStatus()
            connectionError = nil
            let newEvents: [AuditEvent] = try await request("GET", "/v1/events")
            try await registerClient()
            clients = try await request("GET", "/v1/clients")
            await flushProgress()
            #if !os(tvOS)
            await DownloadQueue.shared.syncProgress()
            #endif
            let receivedHistory: [PlaybackHistory] = try await request("GET", "/v1/playback")
            guard identity == profileIdentity else { return }
            history = receivedHistory
            if Date().timeIntervalSince(lastHistoryFetch) >= 30 {
                transferHistory = try await request("GET", "/v1/rates")
                metadata = try await request("GET", "/v1/metadata")
                lastHistoryFetch = Date()
            }
            #if !os(tvOS)
            DownloadQueue.shared.importSharedProgress()
            #endif
            do {
                let received: PendingLibraryOperations = try await request("GET", "/v1/library/operations")
                guard identity == profileIdentity else { return }
                libraryOperations = received.operations
                libraryOperationsError = nil
            } catch {
                guard identity == profileIdentity else { return }
                libraryOperationsError = error.localizedDescription
            }
            do {
                let received: [Torrent] = try await request("GET", "/v1/torrents")
                guard identity == profileIdentity else { return }
                torrents = etaSmoother.update(received, server: server, now: Date().timeIntervalSince1970)
                    .sorted { $0.addedDate > $1.addedDate }
                torrentError = nil
            } catch {
                guard identity == profileIdentity else { return }
                torrentError = error.localizedDescription
            }
            receiveActivity(newEvents)
            events = newEvents.reversed()
            lastUpdated = Date()
            connectionError = nil
        } catch { connectionError = error.localizedDescription }
    }
    @Published var continueWatchingUndo: (id: String, profile: String)?
    func setContinueWatchingHidden(_ id: String, hidden: Bool) async {
        let identity=profileIdentity
        do {
            let _: Empty = try await request("POST", "/v1/library/continue-watching", body: ["title_id":id,"hidden":hidden])
            guard identity == profileIdentity else { return }
            continueWatchingUndo = hidden ? (id,identity) : nil
            await refresh()
        } catch { actionError = error.localizedDescription }
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
        let key = "activity-watermark:" + server
        let last = UserDefaults.standard.object(forKey: key) as? Double
        let newest = incoming.map(\.time).max() ?? Date().timeIntervalSince1970
        UserDefaults.standard.set(max(last ?? 0, newest), forKey: key)
        guard let last, UserDefaults.standard.object(forKey: "activityAlerts") as? Bool ?? true else { return }
        let recent = incoming.filter { $0.time > last && $0.time > Date().timeIntervalSince1970 - 86400 && ["download_complete", "seeding_ratio_reached"].contains($0.kind) }.sorted { $0.time < $1.time }
        if recent.count > 3 {
            notify("Torrent activity", detail: "\(recent.count) downloads or seeding goals completed.")
        } else {
            for event in recent {
                notify(event.kind == "download_complete" ? "Download complete" : "Seeding goal reached", detail: event.name ?? "", torrentHash: event.hash)
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
    /// Request native virtual MP4 tracks without copying the audiovisual stream.
    func mediaPlaybackLink(asset: LibraryAsset, titleID: String, startSession: Bool = true, progress: @escaping (String) -> Void = { _ in }) async throws -> PlaybackLink {
        let serverAtStart = server
        let profileAtStart = profileIdentity
        let base = "/v1/library/assets/\(asset.id)"
        var body: [String: Any] = startSession ? ["client_id": clientId] : [:]
        var ids: [String] = []
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
        let prepared: Job = try await request("POST", base + "/virtual-mp4", body: ["subtitle_jobs": ids])
        if prepared.state != "direct" {
            body["virtual_job"] = try await wait(prepared, route: "/v1/virtual-jobs/").id
        }
        try Task.checkCancellation()
        guard server == serverAtStart && profileIdentity == profileAtStart else { throw CancellationError() }
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
    var symbol: String { avatarSymbol.flatMap { $0.isEmpty ? nil : $0 } ?? String(name.prefix(1)).uppercased() }
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
            } else {
                content.id(store.profileIdentity)
            }
        }
        .overlay(alignment: .topTrailing) {
            if store.configured && (!store.profileReady || store.choosingProfile) {
                InterfaceLanguagePicker().padding(16)
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
            .foregroundStyle(profileColor(profile.textColor ?? "#70E8C2"))
            .frame(width: size, height: size)
            .background(profileColor(profile.backgroundColor ?? "#243A38"), in: Circle())
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
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 22) {
                Text("Who's watching?").font(.largeTitle.bold())
                if store.loadingProfiles || working { ProgressView() }
                if let error = store.profileError {
                    Text(L10n.text(error)).foregroundStyle(.orange)
                    Button("Try again") { Task { await store.loadProfiles() } }
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
                                if person.id == store.activeProfile?.id { Image(systemName: "checkmark.circle.fill").foregroundStyle(Theme.mint) }
                            }.padding(16).contentShape(RoundedRectangle(cornerRadius: 18))
                        }.buttonStyle(.bordered)
                        Button { editing = person } label: { Image(systemName: "pencil").frame(width: 44, height: 44) }
                            .accessibilityLabel("Edit profile")
                    }
                }
                Button("Create user") { creating = true }.libraryActionStyle()
                if store.profileReady { Button("Back to library") { store.choosingProfile = false } }
            }.disabled(working).padding(40).frame(maxWidth: 760).frame(maxWidth: .infinity)
        }.background(Theme.canvas)
            .task { await store.loadProfiles() }
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
        _foreground = State(initialValue: profile?.textColor ?? "#70E8C2")
        _background = State(initialValue: profile?.backgroundColor ?? "#243A38")
    }
    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 22) {
                    ProfileAvatar(profile: UserProfile(id: "preview", name: name, avatarSymbol: symbol, textColor: foreground, backgroundColor: background), size: 100)
                        .frame(maxWidth: .infinity)
                    TextField("First name", text: $name).profileFieldStyle()
                    TextField("Initial or emoji", text: $symbol).profileFieldStyle()
                        .onChange(of: symbol) { _, value in symbol = String(value.prefix(1)) }
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
                                    try await store.saveProfile(id: profile?.id, name: name.trimmingCharacters(in: .whitespacesAndNewlines), symbol: symbol, textColor: foreground, backgroundColor: background)
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
    private let colors = ["#FFFFFF", "#70E8C2", "#F5C974", "#F18FA8", "#A89CF5", "#7DC6FF", "#243A38", "#18202B", "#4C2452", "#6A3434"]
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(LocalizedStringKey(title)).font(.headline)
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 44))], spacing: 12) {
                ForEach(colors, id: \.self) { value in
                    Button { hex = value } label: {
                        Circle().fill(profileColor(value)).frame(width: 36, height: 36)
                            .overlay { Circle().stroke(hex == value ? Theme.mint : .gray.opacity(0.4), lineWidth: hex == value ? 4 : 1) }
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
