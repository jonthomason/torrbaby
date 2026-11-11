#if !os(tvOS)
import Foundation
import CryptoKit
import ImageIO
import UniformTypeIdentifiers
import SwiftUI
#if os(iOS)
import UIKit
#endif

@MainActor
final class DownloadQueue: NSObject, ObservableObject, URLSessionDownloadDelegate {
    static let shared = DownloadQueue()
    static let sessionID = "org.torrbaby.offline-transfers"
    @Published private(set) var items: [DownloadStorage.Item] = []
    @Published private(set) var received: [UUID: Int64] = [:]
    @Published var error: String?
    @Published private(set) var used: Int64 = 0
    @Published private(set) var policy = DownloadStorage.Policy()
    private var storage: DownloadStorage?
    private weak var store: Store?
    private var tasks: [UUID: URLSessionDownloadTask] = [:]
    private var restored = false
    private var rerouting: Set<UUID> = []
    private var session: URLSession!
    nonisolated private let staging: URL
    var backgroundCompletion: (() -> Void)? { didSet { completeBackgroundEvents() } }
    private var finishing = 0
    private var backgroundEventsFinished = false
    private var syncingProgress = false
    private var lastUsageRefresh = Date.distantPast

    init(root requestedRoot: URL? = nil, configuration: URLSessionConfiguration? = nil) {
        let root = requestedRoot ?? FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("Torrbaby/Offline", isDirectory: true)
        staging = root.appendingPathComponent("partial", isDirectory: true)
        super.init()
        do { storage = try DownloadStorage(root: root); reload() }
        catch { self.error = error.localizedDescription }
        let config = configuration ?? URLSessionConfiguration.background(withIdentifier: Self.sessionID)
        config.isDiscretionary = false
        config.sessionSendsLaunchEvents = true
        config.httpMaximumConnectionsPerHost = 1
        session = URLSession(configuration: config, delegate: self, delegateQueue: .main)
        Task {
            let existing = await session.allTasks
            for task in existing {
                guard let id = task.taskDescription.flatMap(UUID.init(uuidString:)),
                      let item = items.first(where: { $0.id == id }), item.state == .transferring,
                      let download = task as? URLSessionDownloadTask else { task.cancel(); continue }
                tasks[id] = download
                received[id] = max(0, download.countOfBytesReceived)
            }
            for item in items where item.state == .transferring && tasks[item.id] == nil {
                try? storage?.pause(item.id, received: item.received, error: "Transfer interrupted. Tap resume.")
            }
            restored = true; reload(); connectionChanged()
        }
    }
    func attach(_ store: Store) { self.store = store; connectionChanged() }
    func connectionChanged() {
        guard restored else { return }
        for (id, task) in tasks {
            guard items.first(where: { $0.id == id })?.state == .transferring else { continue }
            if store?.connected == true {
                if let base = try? store?.endpoint("/").absoluteString,
                   let url = (task.currentRequest ?? task.originalRequest)?.url?.absoluteString,
                   !url.hasPrefix(base) {
                    pause(id, requeue: true)
                    continue
                }
                if task.state == .suspended { task.resume() }
            } else if task.state == .running {
                task.suspend()
            }
        }
        pump()
    }
    func reload() {
        guard let storage else { return }
        items = storage.items.map { $0.forProfile(store?.activeProfile?.id ?? "") }; policy = storage.policy
        do { used = try storage.accountedBytes(received: received) } catch { self.error = error.localizedDescription }
    }
    func setPolicy(_ next: DownloadStorage.Policy) {
        do {
            try storage?.setPolicy(next); reload()
            // Re-admit active requests under the new limits and network policy; a plain pause
            // left the transfer stopped until it was tapped again.
            for id in Array(tasks.keys) { pause(id, requeue: true) }
        } catch { self.error = error.localizedDescription }
    }
    func enqueue(_ asset: LibraryAsset, title: LibraryTitle) async {
        guard let store, store.connected, let storage else { return }
        let server = store.server, identity = store.profileIdentity
        if let existing=items.first(where: { $0.server == server && $0.assetID == asset.id && $0.state != .removed }) {
            if existing.state == .failed { resume(existing.id) }
            return
        }
        do {
            struct Info: Decodable { let name: String; let size: Int64; let etag: String }
            let link = try await store.mediaPlaybackLink(asset: asset, titleID: title.id, startSession: false)
            let virtualJob = URLComponents(string: link.path)?.queryItems?.first(where: { $0.name == "virtual" })?.value
            let info: Info = try await store.request("POST", "/v1/library/assets/\(asset.id)/download-info", body: virtualJob.map { ["virtual_job": $0] } ?? [:])
            guard store.server == server, store.profileIdentity == identity else { return }
            let item = try storage.enqueue(server: server, assetID: asset.id, titleID: title.id,
                                    title: title.title, name: asset.name ?? info.name,
                                    season: asset.season, episode: asset.episode, revision: info.etag,
                                    size: info.size, fileExtension: URL(fileURLWithPath: info.name).pathExtension, virtualJob: virtualJob)
            try storage.rememberEpisodeOrder(item.id, episodes: title.episodeOrder)
            try storage.importProgress(item.id, profile: store.activeProfile?.id ?? "", position: asset.position, duration: asset.duration,
                                       watched: asset.watched, recordedAt: Date(timeIntervalSince1970: asset.lastPlayed))
            reload(); pump()
            await cachePoster(for: item)
        } catch { self.error = error.localizedDescription }
    }
    func artworkURL(_ id: UUID) -> URL? {
        guard let storage, let key = items.first(where: { $0.id == id })?.posterKey else { return nil }
        return storage.posterURL(key)
    }
    private func cachePoster(for item: DownloadStorage.Item) async {
        guard let store, store.connected, let storage, store.server == item.server else { return }
        let key = SHA256.hash(data: Data((item.server + "\n" + item.titleID).utf8)).map { String(format: "%02x", $0) }.joined()
        do {
            if let url = storage.posterURL(key), let data = try? Data(contentsOf: url) {
                try storage.savePoster(item.id, key: key, data: data); reload(); return
            }
            struct Artwork: Decodable { let data: String? }
            let artwork: Artwork = try await store.request("GET", "/v1/library/artwork/\(item.titleID)/poster")
            guard let encoded = artwork.data, encoded.utf8.count <= 12_000_000,
                  let bytes = Data(base64Encoded: encoded),
                  let source = CGImageSourceCreateWithData(bytes as CFData, nil),
                  let thumbnail = CGImageSourceCreateThumbnailAtIndex(source, 0, [
                    kCGImageSourceCreateThumbnailFromImageAlways: true,
                    kCGImageSourceThumbnailMaxPixelSize: 480,
                    kCGImageSourceCreateThumbnailWithTransform: true
                  ] as CFDictionary) else { return }
            let output = NSMutableData()
            guard let destination = CGImageDestinationCreateWithData(output, UTType.jpeg.identifier as CFString, 1, nil) else { return }
            CGImageDestinationAddImage(destination, thumbnail, [kCGImageDestinationLossyCompressionQuality: 0.8] as CFDictionary)
            guard CGImageDestinationFinalize(destination) else { return }
            try storage.savePoster(item.id, key: key, data: output as Data)
            reload()
        } catch { /* Artwork is optional; the media transfer continues independently. */ }
    }
    private func pump() {
        guard restored, tasks.isEmpty, let store, store.connected, let storage else { return }
        guard let item = items.first(where: { $0.state == .queued && $0.server == store.server }) else { return }
        do {
            try storage.markTransferring(item.id)
            let suffix = item.virtualJob.map { "?virtual=" + $0 } ?? ""
            var request = URLRequest(url: try store.endpoint("/v1/library/assets/\(item.assetID)/download" + suffix))
            request.setValue("Bearer " + store.token, forHTTPHeaderField: "Authorization")
            request.setValue(item.revision, forHTTPHeaderField: "If-Match")
            request.allowsCellularAccess = !policy.wifiOnly
            request.allowsExpensiveNetworkAccess = !policy.wifiOnly
            request.timeoutInterval = 120
            let context = Data(SHA256.hash(data: Data("\(store.activeServer ?? store.server)|\(store.token)|\(policy.wifiOnly)".utf8)))
            let task: URLSessionDownloadTask
            if (try? Data(contentsOf: storage.resumeContextURL(item.id))) == context,
               let data = try? Data(contentsOf: storage.resumeURL(item.id)) {
                task = session.downloadTask(withResumeData: data)
                received[item.id] = item.received
            } else {
                try? FileManager.default.removeItem(at: storage.resumeURL(item.id))
                try storage.resetTransferBytes(item.id)
                received[item.id] = 0
                task = session.downloadTask(with: request)
            }
            try context.write(to: storage.resumeContextURL(item.id), options: .atomic)
            task.taskDescription = item.id.uuidString
            tasks[item.id] = task; reload(); task.resume()
        } catch {
            try? storage.pause(item.id, received: item.received, error: error.localizedDescription)
            reload()
        }
    }
    func pause(_ id: UUID, requeue: Bool = false) {
        if requeue { rerouting.insert(id) } else { rerouting.remove(id) }
        guard let storage, let item = items.first(where: { $0.id == id }) else { return }
        do { try storage.pause(id, received: received[id] ?? item.received) }
        catch { self.error = error.localizedDescription; return }
        reload()
        guard let task = tasks[id] else {
            if rerouting.remove(id) != nil { try? storage.queue(id); reload(); pump() }
            return
        }
        task.cancel { [weak self] data in
            DispatchQueue.main.async {
                guard let self, let storage = self.storage else { return }
                if let data, self.items.first(where: { $0.id == id })?.state == .paused {
                    do { try data.write(to: storage.resumeURL(id), options: .atomic) }
                    catch { self.error = error.localizedDescription }
                }
                if self.tasks[id] === task { self.tasks.removeValue(forKey: id) }
                if self.rerouting.remove(id) != nil { try? storage.queue(id) }
                self.reload(); self.pump()
            }
        }
    }
    func resume(_ id: UUID) {
        guard tasks[id] == nil else { return }
        do { try storage?.queue(id); reload(); pump() }
        catch { self.error = error.localizedDescription }
    }
    func remove(_ id: UUID) {
        rerouting.remove(id)
        guard let storage else { return }
        // Persist removal before cancelling so late delegate callbacks cannot restore it.
        do { try storage.remove(id) }
        catch { self.error = error.localizedDescription; return }
        tasks[id]?.cancel(); tasks.removeValue(forKey: id); received.removeValue(forKey: id)
        reload(); pump()
    }
    func pin(_ id: UUID, keep: Bool) {
        do { try storage?.pin(id, keep); reload() } catch { self.error = error.localizedDescription }
    }
    func playback(_ id: UUID) -> Playback? {
        guard let storage, let item = items.first(where: { $0.id == id && $0.state == .ready }),
              FileManager.default.fileExists(atPath: storage.mediaURL(item).path) else { return nil }
        return Playback(sessionId: nil, resumePosition: item.watched ? 0 : item.position,
                        url: storage.mediaURL(item), name: item.name, localDownloadID: id,
                        libraryAssetID: item.assetID, libraryTitleID: item.titleID)
    }
    func readyDownload(assetID: String, server: String) -> UUID? {
        items.first(where: { $0.assetID == assetID && $0.server == server && playback($0.id) != nil })?.id
    }
    func preparePlayback(_ id: UUID) async -> Playback? {
        if let store, let storage, store.connected,
           let item = items.first(where: { $0.id == id && $0.server == store.server && $0.state == .ready }) {
            let server = store.server, identity = store.profileIdentity
            struct Progress: Decodable { let position: Double; let duration: Double; let watched: Bool; let lastPlayed: Double }
            do {
                let progress: Progress = try await store.request("GET", "/v1/library/assets/\(item.assetID)/progress", timeout: 3)
                if store.server == server && store.profileIdentity == identity {
                    try storage.importProgress(id, profile: store.activeProfile?.id ?? "", position: progress.position, duration: progress.duration,
                                               watched: progress.watched, recordedAt: Date(timeIntervalSince1970: progress.lastPlayed))
                    reload()
                }
            } catch { /* Local playback remains available when the server is unreachable. */ }
        }
        return playback(id)
    }
    func isEpisode(_ playback: Playback) -> Bool {
        if let id = playback.localDownloadID { return items.first(where: { $0.id == id })?.episode != nil }
        return store?.library?.titles.first(where: { $0.id == playback.libraryTitleID })?.assets.first(where: { $0.id == playback.libraryAssetID })?.episode != nil
    }
    func autoplayStopMessage(after playback: Playback) -> String {
        let local = items.first { $0.id == playback.localDownloadID }
        let server = local?.server ?? store?.server
        let titleID = local?.titleID ?? playback.libraryTitleID
        let title = store?.server == server ? store?.library?.titles.first { $0.id == titleID } : nil
        let current = title?.assets.first { $0.id == playback.libraryAssetID }
        guard let season = local?.season ?? current?.season, let episode = local?.episode ?? current?.episode else { return "Playback finished" }
        let known = (title?.episodeOrder ?? local?.episodeOrder ?? []) + items.filter { $0.server == server && $0.titleID == titleID }.compactMap { item -> EpisodeNumber? in
            guard let season = item.season, let episode = item.episode else { return nil }
            return EpisodeNumber(season: season, episode: episode)
        }
        let number = EpisodeNumber(season: season, episode: episode)
        if let gap = number.missingMessage(in: known) { return gap }
        if let next = number.next(in: known) {
            if store?.server != server || store?.canStream != true { return "Download \(next.label) to continue" }
            return title?.autoplayStopMessage(after: playback.libraryAssetID ?? "") ?? "Couldn’t start \(next.label)"
        }
        return "No more episodes available"
    }
    private enum NextSelection { case local(UUID); case stream(LibraryTitle, LibraryAsset) }
    func hasNextEpisode(after playback: Playback) -> Bool { nextSelection(after: playback) != nil }
    private func nextSelection(after currentPlayback: Playback) -> NextSelection? {
        let local = items.first { $0.id == currentPlayback.localDownloadID }
        let titleID = local?.titleID ?? currentPlayback.libraryTitleID
        let server = local?.server ?? store?.server
        let title = store?.server == server ? store?.library?.titles.first { $0.id == titleID } : nil
        let currentAsset = title?.assets.first { $0.id == currentPlayback.libraryAssetID }
        guard let season = local?.season ?? currentAsset?.season,
              let episode = local?.episode ?? currentAsset?.episode, let server else { return nil }
        let localSeries = items.filter { $0.server == server && $0.titleID == titleID }
        let catalogOrder = title?.episodeOrder ?? local?.episodeOrder
        let known = (catalogOrder ?? []) + localSeries.compactMap { item -> EpisodeNumber? in
            guard let season = item.season, let episode = item.episode else { return nil }
            return EpisodeNumber(season: season, episode: episode)
        }
        guard let number = EpisodeNumber(season: season, episode: episode).next(in: known, allowSeasonChange: !(catalogOrder ?? []).isEmpty) else { return nil }
        if let next = localSeries.first(where: { $0.season == number.season && $0.episode == number.episode && playback($0.id) != nil }) {
            return .local(next.id)
        }
        guard let store, store.canStream, store.server == server, let title,
              let asset = title.assets.first(where: { $0.role != "extra" && $0.season == number.season && $0.episode == number.episode && $0.complete }) else { return nil }
        return .stream(title, asset)
    }
    func nextPlayback(after currentPlayback: Playback) async throws -> Playback? {
        guard let selection = nextSelection(after: currentPlayback) else { return nil }
        switch selection {
        case .local(let id): return await preparePlayback(id)
        case .stream(let title, let asset):
        guard let store, store.canStream else { throw APIError(message: "Reconnect to your library to play the next episode.") }
        let identity = store.profileIdentity
        await store.flushProgress()
        try Task.checkCancellation()
        try await store.registerClient()
        let link = try await store.mediaPlaybackLink(asset: asset, titleID: title.id)
        guard store.profileIdentity == identity else { throw CancellationError() }
        return Playback(sessionId: link.sessionId, resumePosition: link.resumePosition ?? 0,
                        url: try store.endpoint(link.path), name: asset.name ?? title.title,
                        libraryAssetID: asset.id, libraryTitleID: title.id)
        }
    }
    @discardableResult
    func report(_ id: UUID, profile: String, position: Double, duration: Double, reachedEnd: Bool, watchedEligible: Bool = true) -> Bool {
        guard let storage, position.isFinite, duration.isFinite, duration > 0, position >= 0 else { return false }
        do {
            try storage.recordPlayback(id, profile: profile, position: position, duration: duration, reachedEnd: reachedEnd, watchedEligible: watchedEligible)
            reload()
            Task { await syncProgress() }
            return true
        } catch { self.error = error.localizedDescription; return false }
    }
    /// Refresh downloaded copies before the device leaves the network, including
    /// episodes never opened locally. Timestamp checks preserve newer offline work.
    func importSharedProgress() {
        guard let store, let storage, let library = store.library else { return }
        let assets = library.titles.flatMap(\.assets)
        let byID = Dictionary(assets.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        do {
            for item in storage.items where item.server == store.server && item.state != .removed && item.state != .removing {
                if let title = library.titles.first(where: { $0.id == item.titleID }) {
                    try storage.rememberEpisodeOrder(item.id, episodes: title.episodeOrder)
                }
                guard let asset = byID[item.assetID] else { continue }
                try storage.importProgress(item.id, profile: store.activeProfile?.id ?? "", position: asset.position, duration: asset.duration,
                                           watched: asset.watched, recordedAt: Date(timeIntervalSince1970: asset.lastPlayed))
            }
            reload()
        } catch { self.error = error.localizedDescription }
    }
    func syncProgress() async {
        guard !syncingProgress, let store, store.configured, store.connected, let storage else { return }
        syncingProgress = true
        defer { syncingProgress = false }
        let server = store.server, client = store.clientId
        do {
            try await store.registerClient()
            for item in storage.items where item.server == server {
              for (profile, value) in item.allViewing where value.progressNeedsSync {
                guard store.server == server && store.clientId == client else { return }
                guard let event = value.progressEventID, let recorded = value.progressRecordedAt else { continue }
                do {
                    struct Acknowledgment: Decodable { let acknowledged: Bool }
                    let reply: Acknowledgment = try await store.request("POST", "/v1/playback/offline", body: [
                        "event_id": event.uuidString, "client_id": client, "asset_id": item.assetID,
                        "profile_id": profile, "position": value.position, "duration": value.duration,
                        "recorded_at": recorded.timeIntervalSince1970, "reached_end": value.playbackFinished, "watched_eligible": value.watched
                    ])
                    guard store.server == server && store.clientId == client else { return }
                    if reply.acknowledged { try storage.acknowledgeProgress(item.id, profile: profile, eventID: event) }
                } catch {
                    // One removed catalog asset must not block other reports.
                    continue
                }
              }
            }
            reload()
        } catch {
            // Keep the durable report for the next refresh/reconnection. Playback
            // and local deletion must not depend on network availability.
        }
    }
    func finished(_ id: UUID) {
        // Shared files remain until an explicit removal when several people use this library.
        guard let store, store.profiles.count == 1, let profile = store.activeProfile?.id else { return }
        do { try storage?.removeAfterPlayback(id, profile: profile); reload(); pump() }
        catch { self.error = error.localizedDescription }
    }
    nonisolated func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask,
                                didWriteData bytesWritten: Int64, totalBytesWritten: Int64, totalBytesExpectedToWrite: Int64) {
        guard let id = downloadTask.taskDescription.flatMap(UUID.init(uuidString:)) else { return }
        DispatchQueue.main.async {
            guard self.tasks[id] === downloadTask else { return }
            // Publishing every callback re-rendered each download control many times a second.
            guard Date().timeIntervalSince(self.lastUsageRefresh) >= 1 else { return }
            self.received[id] = totalBytesWritten
            self.lastUsageRefresh = Date()
            self.reload()
            if let storage = self.storage {
                do {
                    if try storage.availableBytes() < self.policy.freeSpaceBytes || !self.policy.permits(used: self.used) {
                        self.pause(id)
                    }
                } catch { self.error = error.localizedDescription; self.pause(id) }
            }
        }
    }
    nonisolated func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didFinishDownloadingTo location: URL) {
        guard let id = downloadTask.taskDescription.flatMap(UUID.init(uuidString:)) else { return }
        let destination = staging.appendingPathComponent(UUID().uuidString + ".completed")
        let response = downloadTask.response as? HTTPURLResponse
        let move: Result<URL, Error> = Result {
            // The delegate's temporary file disappears on return; move it synchronously.
            try FileManager.default.moveItem(at: location, to: destination)
            return destination
        }
        DispatchQueue.main.async {
            self.finishing += 1
            defer {
                self.finishing -= 1
                if self.tasks[id] === downloadTask { self.tasks.removeValue(forKey: id) }
                self.reload(); self.pump(); self.completeBackgroundEvents()
            }
            do {
                let file = try move.get()
                defer { try? FileManager.default.removeItem(at: file) }
                // Background callbacks can precede task restoration, but a known
                // replacement task must never be finalized by an older callback.
                if let current = self.tasks[id], current !== downloadTask { return }
                guard let response, (200...299).contains(response.statusCode) else {
                    throw APIError(message: response?.statusCode == 412 ? "The file changed. Remove this download and add it again." : "Download failed. Reconnect and try again.")
                }
                try self.storage?.adopt(id, temporary: file, revision: response.value(forHTTPHeaderField: "ETag") ?? "")
                self.received.removeValue(forKey: id)
                if let storage = self.storage { try? FileManager.default.removeItem(at: storage.resumeURL(id)) }
            } catch {
                self.received[id] = 0
                if let storage = self.storage { try? FileManager.default.removeItem(at: storage.resumeURL(id)) }
                try? self.storage?.pause(id, received: 0, error: error.localizedDescription)
            }
        }
    }
    nonisolated func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        guard let error, let id = task.taskDescription.flatMap(UUID.init(uuidString:)) else { return }
        let resume = (error as NSError).userInfo[NSURLSessionDownloadTaskResumeData] as? Data
        DispatchQueue.main.async {
            guard self.tasks[id] === task else { return }
            // A requested pause owns its resume-data callback and task removal.
            guard self.items.first(where: { $0.id == id })?.state == .transferring else { return }
            let code = (error as NSError).code
            let lostConnection = self.store?.connected != true ||
                ((error as NSError).domain == NSURLErrorDomain &&
                 [NSURLErrorTimedOut, NSURLErrorCannotFindHost, NSURLErrorCannotConnectToHost,
                  NSURLErrorNetworkConnectionLost, NSURLErrorDNSLookupFailed,
                  NSURLErrorNotConnectedToInternet, NSURLErrorInternationalRoamingOff,
                  NSURLErrorDataNotAllowed].contains(code))
            if lostConnection { self.store?.connectionError = error.localizedDescription }
            if let storage = self.storage {
                if let resume { try? resume.write(to: storage.resumeURL(id), options: .atomic) }
                else {
                    try? FileManager.default.removeItem(at: storage.resumeURL(id))
                    self.received[id] = 0
                }
                try? storage.pause(id, received: self.received[id] ?? 0,
                                   error: lostConnection ? nil : "Download interrupted. Tap resume.")
                if lostConnection { try? storage.queue(id) }
            }
            self.tasks.removeValue(forKey: id); self.reload(); self.pump()
        }
    }
    nonisolated func urlSessionDidFinishEvents(forBackgroundURLSession session: URLSession) {
        DispatchQueue.main.async { self.backgroundEventsFinished = true; self.completeBackgroundEvents() }
    }
    private func completeBackgroundEvents() {
        guard finishing == 0, backgroundEventsFinished, let completion = backgroundCompletion else { return }
        backgroundCompletion = nil; backgroundEventsFinished = false; completion()
    }
}

#if os(iOS)
final class DownloadAppDelegate: NSObject, UIApplicationDelegate {
    func application(_ application: UIApplication, handleEventsForBackgroundURLSession identifier: String,
                     completionHandler: @escaping () -> Void) {
        guard identifier == DownloadQueue.sessionID else { completionHandler(); return }
        DownloadQueue.shared.backgroundCompletion = completionHandler
    }
}
#endif
#endif
