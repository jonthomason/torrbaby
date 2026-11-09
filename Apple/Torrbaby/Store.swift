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
    func reportProgress(sessionId: String, sequence: Int, position: Double, duration: Double, ended: Bool) {
        guard position.isFinite, duration.isFinite, duration > 0 else { return }
        var pending = UserDefaults.standard.dictionary(forKey: pendingKey) ?? [:]
        pending[sessionId] = ["session_id": sessionId, "sequence": sequence, "position": position, "duration": duration, "ended": ended]
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
        return try decoder.decode(T.self, from: data)
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
            await refreshStatus()
            let received: [Torrent] = try await request("GET", "/v1/torrents")
            let newTorrents = etaSmoother.update(received, server: server, now: Date().timeIntervalSince1970)
            torrents = newTorrents.sorted { $0.addedDate > $1.addedDate }
            connectionError = nil
            let newEvents: [AuditEvent] = try await request("GET", "/v1/events")
            try await registerClient()
            clients = try await request("GET", "/v1/clients")
            await flushProgress()
            #if !os(tvOS)
            await DownloadQueue.shared.syncProgress()
            #endif
            history = try await request("GET", "/v1/playback")
            do {
                library = try await request("GET", "/v1/library")
                libraryError = nil
            } catch { libraryError = error.localizedDescription }
            if Date().timeIntervalSince(lastHistoryFetch) >= 30 {
                transferHistory = try await request("GET", "/v1/rates")
                metadata = try await request("GET", "/v1/metadata")
                lastHistoryFetch = Date()
            }
            #if !os(tvOS)
            DownloadQueue.shared.importSharedProgress()
            #endif
            torrents = newTorrents.sorted { $0.addedDate > $1.addedDate }
            receiveActivity(newEvents)
            events = newEvents.reversed()
            lastUpdated = Date()
            connectionError = nil
        } catch { connectionError = error.localizedDescription }
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
        perform { try await self.send("POST", "/v1/torrents/\(torrent.id)/\(action)", body: body) }
    }
    func toggleAutomation() {
        guard let status else { return }
        perform { try await self.send("PATCH", "/v1/settings", body: ["paused": !status.settings.paused]) }
    }
}

extension Store {
    /// Use native HLS renditions when a subtitle preference requires text delivery.
    func mediaPlaybackLink(asset: LibraryAsset, titleID: String, progress: @escaping (String) -> Void = { _ in }) async throws -> PlaybackLink {
        let serverAtStart = server
        let base = "/v1/library/assets/\(asset.id)"
        var body: [String: Any] = ["client_id": clientId]
        do {
            let preferences: SubtitlePreferenceResponse = try await request("GET", deviceSubtitlePath)
            if preferences.effective.enabled {
                progress("Preparing subtitles…")
                struct Track: Decodable {
                    let track: Int; let codec: String; let language: String?; let extractionSupported: Bool
                    var normalizedLanguage: String? {
                        let code = (language ?? "").lowercased()
                        if code == "eng" || code.hasPrefix("en") { return "en" }
                        if code == "zho" || code == "chi" || code.hasPrefix("zh") { return "zh-Hans" }
                        return nil
                    }
                }
                struct Inventory: Decodable { let embeddedTracks: [Track] }
                struct Job: Decodable { let id: String; let state: String; let error: String? }
                let inventory: Inventory = try await request("GET", base + "/subtitles")
                let tracks = inventory.embeddedTracks.filter { $0.extractionSupported && $0.codec == "tx3g" }
                guard let source = tracks.first(where: { $0.normalizedLanguage != nil }) ?? tracks.first else {
                    throw APIError(message: "Translated subtitles aren’t available for this file yet.")
                }
                func wait(_ job: Job, route: String) async throws -> Job {
                    var result = job
                    let deadline = Date().addingTimeInterval(1200)
                    while result.state == "queued" || result.state == "running" || result.state == "preparing" {
                        try Task.checkCancellation()
                        guard self.server == serverAtStart else { throw CancellationError() }
                        guard Date() < deadline else { throw APIError(message: "Subtitles are still preparing. Try again shortly.") }
                        try await Task.sleep(for: .seconds(2))
                        result = try await self.request("GET", route + result.id)
                    }
                    guard result.state == "ready" else { throw APIError(message: result.error ?? "Subtitles couldn’t be prepared.") }
                    return result
                }
                var originalBody: [String: Any] = ["track": source.track]
                if let language = source.normalizedLanguage { originalBody["source"] = language }
                let original: Job = try await request("POST", base + "/subtitles/prepare", body: originalBody)
                let readyOriginal = try await wait(original, route: "/v1/subtitle-jobs/")
                var ids = [readyOriginal.id]
                let target = preferences.effective.language
                if target != "original" && target != source.normalizedLanguage {
                    var translatedBody: [String: Any]
                    if let existing = tracks.first(where: { $0.normalizedLanguage == target }) {
                        translatedBody = ["track": existing.track, "source": target]
                    } else {
                        let ready: MediaTrackInfo = try await request("GET", base + "/media-info")
                        guard let translated = ready.prepared.first(where: { $0.language == target && $0.generated && $0.state == "ready" }) else {
                            throw APIError(message: "Translated subtitles are not ready yet. The server prepares them in the background.")
                        }
                        ids.append(translated.id)
                        translatedBody = [:]
                    }
                    if !translatedBody.isEmpty {
                        let job: Job = try await request("POST", base + "/subtitles/prepare", body: translatedBody)
                        ids.append(try await wait(job, route: "/v1/subtitle-jobs/").id)
                    }
                }
                progress("Preparing playback…")
                let package: Job = try await request("POST", base + "/hls", body: ["subtitle_jobs": ids])
                body["hls_job"] = try await wait(package, route: "/v1/hls-jobs/").id
            }
        } catch {
            try Task.checkCancellation()
            guard server == serverAtStart else { throw CancellationError() }
            notify("Subtitles unavailable", detail: error.localizedDescription)
        }
        try Task.checkCancellation()
        guard server == serverAtStart else { throw CancellationError() }
        return try await request("POST", base + "/play", body: body)
    }
}

extension Store {
    var deviceSubtitlePath: String { "/v1/devices/\(clientId)/subtitle-preferences" }
}
