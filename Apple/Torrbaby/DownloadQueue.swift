#if !os(tvOS)
import Foundation
import CryptoKit
import ImageIO
import UniformTypeIdentifiers
import SwiftUI
import Network
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
    /// Where each running request's bytes begin in its file: what was already kept when it started.
    private var segmentBase: [UUID: Int64] = [:]
    /// Request bytes at the last checkpoint, so a failed one is not retried at once.
    private var checkpointed: [UUID: Int64] = [:]
    /// While the app is open, what a request has fetched is joined to the kept bytes this
    /// often, so even a transfer the system drops outright loses little.
    static let checkpointBytes: Int64 = {
        #if DEBUG
        // Tests exercise the join path on small files: -downloadCheckpointMB 50.
        let megabytes = UserDefaults.standard.integer(forKey: "downloadCheckpointMB")
        if megabytes > 0 { return Int64(megabytes) << 20 }
        #endif
        return 512 << 20
    }()
    nonisolated private static let files = DispatchQueue(label: "org.torrbaby.offline-files", qos: .utility)
    private var session: URLSession!
    nonisolated private let staging: URL
    var backgroundCompletion: (() -> Void)? { didSet { completeBackgroundEvents() } }
    private var finishing = 0
    private var backgroundEventsFinished = false
    private var syncingProgress = false
    private var lastUsageRefresh = Date.distantPast
    /// Cellular, a Personal Hotspot or a Low Data Mode network. Assumed until the first path
    /// report, so nothing starts before the connection is known to be unmetered.
    @Published private(set) var metered = true
    private let pathMonitor = NWPathMonitor()
    /// Whether cellular, hotspot and Low Data Mode networks may be used right now. On iPhone and
    /// iPad that is a three-hour allowance that also ends on joining Wi-Fi. The Mac has no
    /// cellular of its own, so it does not hold downloads back.
    var meteredAllowed: Bool {
        #if os(iOS)
        return (policy.meteredUntil ?? 0) > Date().timeIntervalSince1970
        #else
        return true
        #endif
    }
    /// Transfers wait for an unmetered connection unless metered use is allowed.
    var waitingForWiFi: Bool { metered && !meteredAllowed }
    static let meteredAllowance: TimeInterval = 3 * 3600
    private var meteredExpiry: Timer?
    /// Allow cellular and hotspot downloads for three hours. Transfers that were started for
    /// Wi-Fi only are restarted so their requests may use cellular.
    func allowMeteredForAWhile() {
        guard var next = storage?.policy else { return }
        next.meteredUntil = Date().addingTimeInterval(Self.meteredAllowance).timeIntervalSince1970
        do { try storage?.setPolicy(next); reload() } catch { self.report(error); return }
        for (id, task) in tasks where (task.originalRequest?.allowsCellularAccess ?? true) == false { pause(id, requeue: true) }
        scheduleMeteredExpiry(); connectionChanged()
    }
    /// End the allowance (asked for, expired, or Wi-Fi joined). A transfer under way keeps its
    /// progress; while the app runs it pauses whenever the network is metered.
    func endMeteredAllowance() {
        meteredExpiry?.invalidate(); meteredExpiry = nil
        guard var next = storage?.policy, next.meteredUntil != nil else { return }
        next.meteredUntil = nil
        do { try storage?.setPolicy(next); reload() } catch { self.report(error); return }
        connectionChanged()
    }
    private func scheduleMeteredExpiry() {
        meteredExpiry?.invalidate(); meteredExpiry = nil
        guard let until = policy.meteredUntil else { return }
        let delay = until - Date().timeIntervalSince1970
        guard delay > 0 else { endMeteredAllowance(); return }
        meteredExpiry = Timer.scheduledTimer(withTimeInterval: delay, repeats: false) { [weak self] _ in
            Task { @MainActor in self?.endMeteredAllowance() }
        }
    }
    func stateLabel(_ item: DownloadStorage.Item) -> String {
        if waitingForWiFi && (item.state == .queued || item.state == .transferring) { return "Waiting for Wi-Fi" }
        return item.state == .transferring ? "Downloading" : item.state.rawValue.capitalized
    }

    init(root requestedRoot: URL? = nil, configuration: URLSessionConfiguration? = nil) {
        let root = requestedRoot ?? FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("Torrbaby/Offline", isDirectory: true)
        staging = root.appendingPathComponent("partial", isDirectory: true)
        super.init()
        do { storage = try DownloadStorage(root: root); reload() }
        catch { self.report(error) }
        let config = configuration ?? URLSessionConfiguration.background(withIdentifier: Self.sessionID)
        config.isDiscretionary = false
        config.sessionSendsLaunchEvents = true
        config.httpMaximumConnectionsPerHost = Self.simultaneous
        session = URLSession(configuration: config, delegate: self, delegateQueue: .main)
        pathMonitor.pathUpdateHandler = { [weak self] path in
            // A VPN tunnel reports the cost of the connection underneath it.
            var metered = path.isExpensive || path.isConstrained || path.usesInterfaceType(.cellular)
            #if DEBUG
            if UserDefaults.standard.bool(forKey: "simulateMeteredNetwork") { metered = true }
            #endif
            DispatchQueue.main.async {
                guard let self, self.metered != metered else { return }
                self.metered = metered
                #if os(iOS)
                // Joining Wi-Fi ends a cellular allowance.
                if !metered && self.policy.meteredUntil != nil { self.endMeteredAllowance() }
                #endif
                self.connectionChanged()
            }
        }
        pathMonitor.start(queue: .global(qos: .utility))
        Task {
            let existing = await session.allTasks
            for task in existing {
                guard let id = task.taskDescription.flatMap(UUID.init(uuidString:)),
                      let item = items.first(where: { $0.id == id }), item.state == .transferring,
                      let download = task as? URLSessionDownloadTask else { task.cancel(); continue }
                tasks[id] = download
                let base = storage.map { Self.readBase($0.baseURL(id)) } ?? 0
                segmentBase[id] = base
                received[id] = base + max(0, download.countOfBytesReceived)
            }
            for item in items where item.state == .transferring && tasks[item.id] == nil {
                // The system dropped the transfer (an app update, a restart, a force quit).
                // It was running, so it goes back in the queue instead of waiting for a tap.
                try? storage?.pause(item.id, received: item.received, error: nil)
                try? storage?.queue(item.id)
            }
            restored = true; reload(); scheduleMeteredExpiry(); connectionChanged()
        }
    }
    func attach(_ store: Store) { self.store = store; connectionChanged() }
    func connectionChanged() {
        guard restored else { return }
        for (id, task) in tasks {
            guard items.first(where: { $0.id == id })?.state == .transferring else { continue }
            if store?.connected == true && !waitingForWiFi {
                if let base = try? store?.endpoint("/").absoluteString,
                   let url = (task.currentRequest ?? task.originalRequest)?.url?.absoluteString,
                   !url.hasPrefix(base) {
                    pause(id, requeue: true)
                    continue
                }
                if task.state == .suspended { task.resume() }
            } else if waitingForWiFi {
                if task.state == .running { task.suspend() }
            } else if task.state == .suspended {
                // The app has lost its own connection, which also happens whenever it leaves
                // the foreground. The system transfer waits for the network by itself and
                // carries on in the background, so it is never held for that.
                task.resume()
            }
        }
        pump()
    }
    func reload() {
        guard let storage else { return }
        items = storage.items.map { $0.forProfile(store?.activeProfile?.id ?? "") }; policy = storage.policy
        do { used = try storage.accountedBytes(received: received) } catch { self.report(error) }
    }
    func setPolicy(_ next: DownloadStorage.Policy) {
        do {
            try storage?.setPolicy(next); reload()
            // Re-admit active requests under the new limits and network policy; a plain pause
            // left the transfer stopped until it was tapped again.
            for id in Array(tasks.keys) { pause(id, requeue: true) }
        } catch { self.report(error) }
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
        } catch { self.report(error) }
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
    /// A request dropped because the route or profile changed under it is not a failure
    /// to show: the queue re-issues it. Only real errors reach the Downloads page.
    private func report(_ error: Error) {
        if error is CancellationError { return }
        let ns = error as NSError
        if ns.domain == NSURLErrorDomain && ns.code == NSURLErrorCancelled { return }
        self.error = error.localizedDescription
    }
    /// Transfers at once. Several small files finish sooner together, and one slow
    /// file no longer holds the rest; more than this mostly splits the same bandwidth.
    static let simultaneous = 3
    private func pump() {
        // Each start marks its item as transferring, so the next pass picks the next one.
        for _ in 0..<Self.simultaneous { if !startNext() { return } }
    }
    private func startNext() -> Bool {
        guard restored, tasks.count < Self.simultaneous, !waitingForWiFi, let store, store.connected, let storage else { return false }
        guard let item = items.first(where: { $0.state == .queued && $0.server == store.server && tasks[$0.id] == nil }) else { return false }
        do {
            try storage.markTransferring(item.id)
            let suffix = item.virtualJob.map { "?virtual=" + $0 } ?? ""
            var request = URLRequest(url: try store.endpoint("/v1/library/assets/\(item.assetID)/download" + suffix))
            request.setValue("Bearer " + store.token, forHTTPHeaderField: "Authorization")
            request.setValue(item.revision, forHTTPHeaderField: "If-Match")
            let allowed = meteredAllowed
            request.allowsCellularAccess = allowed
            request.allowsExpensiveNetworkAccess = allowed
            request.allowsConstrainedNetworkAccess = allowed
            request.timeoutInterval = 120
            let context = Data(SHA256.hash(data: Data("\(store.activeServer ?? store.server)|\(store.token)|\(!allowed)".utf8)))
            let task: URLSessionDownloadTask
            let saved = try? Data(contentsOf: storage.resumeURL(item.id))
            if let saved, (try? Data(contentsOf: storage.resumeContextURL(item.id))) == context {
                // Same address as before: the system continues its own request.
                task = session.downloadTask(withResumeData: saved)
                segmentBase[item.id] = Self.readBase(storage.baseURL(item.id))
                received[item.id] = item.received
            } else {
                // Another address, or nothing to continue: keep what the last request fetched,
                // then ask this address for the rest of the file only.
                if let saved {
                    _ = Self.salvage(resume: saved, partial: storage.partialURL(item.id),
                                     base: Self.readBase(storage.baseURL(item.id)), revision: item.revision)
                }
                try? FileManager.default.removeItem(at: storage.resumeURL(item.id))
                var kept = storage.keptBytes(item.id)
                if kept > item.size { storage.discardKept(item.id); kept = 0 }
                if kept == item.size {
                    try storage.adopt(item.id, temporary: storage.partialURL(item.id), revision: item.revision)
                    storage.discardKept(item.id); received.removeValue(forKey: item.id)
                    reload(); return true
                }
                if kept > 0 { request.setValue("bytes=\(kept)-", forHTTPHeaderField: "Range") }
                try Data(String(kept).utf8).write(to: storage.baseURL(item.id), options: .atomic)
                try storage.setReceived(item.id, kept)
                segmentBase[item.id] = kept; received[item.id] = kept; checkpointed[item.id] = 0
                task = session.downloadTask(with: request)
            }
            try context.write(to: storage.resumeContextURL(item.id), options: .atomic)
            task.taskDescription = item.id.uuidString
            tasks[item.id] = task; reload(); task.resume()
            return true
        } catch {
            try? storage.pause(item.id, received: item.received, error: error.localizedDescription)
            reload()
            return true
        }
    }
    func pause(_ id: UUID, requeue: Bool = false) {
        if requeue { rerouting.insert(id) } else { rerouting.remove(id) }
        guard let storage, let item = items.first(where: { $0.id == id }) else { return }
        do { try storage.pause(id, received: received[id] ?? item.received) }
        catch { self.report(error); return }
        reload()
        guard let task = tasks[id] else {
            if rerouting.remove(id) != nil { try? storage.queue(id); reload(); pump() }
            return
        }
        let partial = storage.partialURL(id), base = segmentBase[id] ?? Self.readBase(storage.baseURL(id)), revision = item.revision
        task.cancel { [weak self] data in
            Self.files.async {
                // Join what this request fetched to the kept bytes, so the next request can
                // come from any address. Failing that, the resume data still serves this one.
                let joined = data.map { Self.salvage(resume: $0, partial: partial, base: base, revision: revision) } ?? false
                DispatchQueue.main.async {
                    guard let self, let storage = self.storage else { return }
                    let state = self.items.first(where: { $0.id == id })?.state
                    if state == nil || state == .removed || state == .removing { storage.discardKept(id) }
                    else if joined || data == nil {
                        try? FileManager.default.removeItem(at: storage.resumeURL(id))
                        let kept = storage.keptBytes(id)
                        self.received[id] = kept; try? storage.setReceived(id, kept)
                    } else if let data, state == .paused {
                        do { try data.write(to: storage.resumeURL(id), options: .atomic) }
                        catch { self.report(error) }
                    }
                    if self.tasks[id] === task { self.tasks.removeValue(forKey: id) }
                    if self.rerouting.remove(id) != nil { try? storage.queue(id) }
                    self.reload(); self.pump()
                }
            }
        }
    }
    nonisolated private static func readBase(_ url: URL) -> Int64 {
        (try? String(contentsOf: url, encoding: .utf8)).flatMap { Int64($0.trimmingCharacters(in: .whitespacesAndNewlines)) } ?? 0
    }
    /// What an interrupted request had fetched sits in a system temporary file that its
    /// resume data names. Join those bytes to the kept bytes; true when they are joined.
    /// The layout of resume data is not documented, so every step is checked and any
    /// doubt leaves the kept bytes untouched (the request's own resume data is then used).
    nonisolated private static func salvage(resume: Data, partial: URL, base: Int64, revision: String) -> Bool {
        var info: [String: Any]?
        if let plain = (try? PropertyListSerialization.propertyList(from: resume, format: nil)) as? [String: Any], plain["$archiver"] == nil {
            info = plain
        } else if let unarchiver = try? NSKeyedUnarchiver(forReadingFrom: resume) {
            unarchiver.requiresSecureCoding = false
            info = unarchiver.decodeObject(forKey: "NSKeyedArchiveRootObjectKey") as? [String: Any]
            unarchiver.finishDecoding()
        }
        guard let info else { return false }
        let plainTag = { (tag: String) in tag.replacingOccurrences(of: "W/", with: "").trimmingCharacters(in: CharacterSet(charactersIn: "\" ")) }
        if let tag = info["NSURLSessionResumeEntityTag"] as? String, !revision.isEmpty, plainTag(tag) != plainTag(revision) { return false }
        guard let fetched = (info["NSURLSessionResumeBytesReceived"] as? NSNumber)?.int64Value, fetched > 0 else { return false }
        var candidates: [URL] = []
        if let path = info["NSURLSessionResumeInfoLocalPath"] as? String { candidates.append(URL(fileURLWithPath: path)) }
        if let name = info["NSURLSessionResumeInfoTempFileName"] as? String, !name.contains("/") {
            candidates.append(URL(fileURLWithPath: NSTemporaryDirectory()).appendingPathComponent(name))
            if let caches = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask).first, let bundle = Bundle.main.bundleIdentifier {
                candidates.append(caches.appendingPathComponent("com.apple.nsurlsessiond/Downloads/\(bundle)/\(name)"))
            }
        }
        guard let piece = candidates.first(where: { (DownloadStorage.fileSize($0) ?? 0) > 0 }),
              let size = DownloadStorage.fileSize(piece) else { return false }
        return (try? DownloadStorage.join(piece: piece, to: partial, offset: base, count: min(size, fetched))) != nil
    }
    func resume(_ id: UUID) {
        guard tasks[id] == nil else { return }
        do { try storage?.queue(id); reload(); pump() }
        catch { self.report(error) }
    }
    func remove(_ id: UUID) {
        rerouting.remove(id)
        guard let storage else { return }
        // Persist removal before cancelling so late delegate callbacks cannot restore it.
        do { try storage.remove(id) }
        catch { self.report(error); return }
        tasks[id]?.cancel(); tasks.removeValue(forKey: id); received.removeValue(forKey: id)
        segmentBase.removeValue(forKey: id); checkpointed.removeValue(forKey: id)
        reload(); pump()
    }
    func pin(_ id: UUID, keep: Bool) {
        do { try storage?.pin(id, keep); reload() } catch { self.report(error) }
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
        } catch { self.report(error); return false }
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
        } catch { self.report(error) }
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
        catch { self.report(error) }
    }
    nonisolated func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask,
                                didWriteData bytesWritten: Int64, totalBytesWritten: Int64, totalBytesExpectedToWrite: Int64) {
        guard let id = downloadTask.taskDescription.flatMap(UUID.init(uuidString:)) else { return }
        DispatchQueue.main.async {
            guard self.tasks[id] === downloadTask else { return }
            // Publishing every callback re-rendered each download control many times a second.
            guard Date().timeIntervalSince(self.lastUsageRefresh) >= 1 else { return }
            self.received[id] = (self.segmentBase[id] ?? 0) + totalBytesWritten
            self.lastUsageRefresh = Date()
            self.reload()
            if totalBytesWritten - (self.checkpointed[id] ?? 0) >= Self.checkpointBytes, self.appIsActive {
                // Bank this request's bytes and carry on with a fresh one from there.
                self.checkpointed[id] = totalBytesWritten
                self.pause(id, requeue: true)
                return
            }
            if let storage = self.storage {
                do {
                    if try storage.availableBytes() < self.policy.freeSpaceBytes || !self.policy.permits(used: self.used) {
                        self.pause(id)
                    }
                } catch { self.report(error); self.pause(id) }
            }
        }
    }
    nonisolated func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didFinishDownloadingTo location: URL) {
        guard let id = downloadTask.taskDescription.flatMap(UUID.init(uuidString:)) else { return }
        let base = Self.readBase(staging.appendingPathComponent(id.uuidString + ".base"))
        let partial = staging.appendingPathComponent(id.uuidString + ".partial")
        // Named for its item and offset, so a piece fetched just before the app stops is joined at next launch.
        let destination = staging.appendingPathComponent("\(id.uuidString).\(base).piece")
        let response = downloadTask.response as? HTTPURLResponse
        let move: Result<URL, Error> = Result {
            // The delegate's temporary file disappears on return; move it synchronously.
            try? FileManager.default.removeItem(at: destination)
            try FileManager.default.moveItem(at: location, to: destination)
            return destination
        }
        DispatchQueue.main.async {
            self.finishing += 1
            let done = {
                self.finishing -= 1
                if self.tasks[id] === downloadTask { self.tasks.removeValue(forKey: id) }
                self.reload(); self.pump(); self.completeBackgroundEvents()
            }
            // The kept bytes survive every failure here; only this request's piece is given up.
            let fail = { (error: Error, keep: Bool) in
                try? FileManager.default.removeItem(at: destination)
                if let storage = self.storage {
                    try? FileManager.default.removeItem(at: storage.resumeURL(id))
                    if !keep { storage.discardKept(id) }
                    let kept = storage.keptBytes(id)
                    self.received[id] = kept
                    try? storage.pause(id, received: kept, error: error.localizedDescription)
                }
                done()
            }
            do {
                let file = try move.get()
                // Background callbacks can precede task restoration, but a known
                // replacement task must never be finalized by an older callback.
                if let current = self.tasks[id], current !== downloadTask { try? FileManager.default.removeItem(at: file); done(); return }
                guard let storage = self.storage, let item = self.items.first(where: { $0.id == id }) else {
                    try? FileManager.default.removeItem(at: file); done(); return
                }
                if response?.statusCode == 412 {
                    // The server's file is a different one now; bytes of the old one cannot be continued.
                    fail(APIError(message: "The file changed. Remove this download and add it again."), false); return
                }
                guard let response, (200...299).contains(response.statusCode) else {
                    throw APIError(message: "Download failed. Reconnect and try again.")
                }
                if let tag = response.value(forHTTPHeaderField: "ETag"), !tag.isEmpty, tag != item.revision {
                    fail(APIError(message: "The file changed. Remove this download and add it again."), false); return
                }
                guard let size = DownloadStorage.fileSize(file) else { throw APIError(message: "Download failed. Reconnect and try again.") }
                // A piece that completes the file from where the kept bytes end is joined to
                // them; a whole file (a server that ignored the range) replaces them.
                let offset: Int64
                if base + size == item.size { offset = base }
                else if size == item.size { offset = 0; try? FileManager.default.removeItem(at: partial) }
                else { throw APIError(message: "Download failed. Reconnect and try again.") }
                let revision = item.revision
                Self.files.async {
                    let joined = Result { try DownloadStorage.join(piece: file, to: partial, offset: offset) }
                    DispatchQueue.main.async {
                        do {
                            _ = try joined.get()
                            try storage.adopt(id, temporary: partial, revision: revision)
                            storage.discardKept(id)
                            self.received.removeValue(forKey: id); self.segmentBase.removeValue(forKey: id); self.checkpointed.removeValue(forKey: id)
                            done()
                        } catch { fail(error, true) }
                    }
                }
            } catch { fail(error, true) }
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
            // The system ended it (an app update, a restart): nothing is wrong, so it carries on.
            let systemEnded = (error as NSError).domain == NSURLErrorDomain && code == NSURLErrorCancelled
                && (error as NSError).userInfo[NSURLErrorBackgroundTaskCancelledReasonKey] != nil
            guard let storage = self.storage, let item = self.items.first(where: { $0.id == id }) else {
                self.tasks.removeValue(forKey: id); self.reload(); self.pump(); return
            }
            let finish = { (joined: Bool) in
                if joined || resume == nil {
                    // Joined to the kept bytes, or nothing recoverable from this request: either
                    // way the file continues from what is kept, from any address.
                    try? FileManager.default.removeItem(at: storage.resumeURL(id))
                    self.received[id] = storage.keptBytes(id)
                } else if let resume { try? resume.write(to: storage.resumeURL(id), options: .atomic) }
                try? storage.pause(id, received: self.received[id] ?? storage.keptBytes(id),
                                   error: lostConnection || systemEnded ? nil : "Download interrupted. Tap resume.")
                if lostConnection || systemEnded { try? storage.queue(id) }
                self.tasks.removeValue(forKey: id); self.reload(); self.pump()
            }
            guard let resume else { finish(false); return }
            let partial = storage.partialURL(id), base = self.segmentBase[id] ?? Self.readBase(storage.baseURL(id)), revision = item.revision
            Self.files.async {
                let joined = Self.salvage(resume: resume, partial: partial, base: base, revision: revision)
                DispatchQueue.main.async { finish(joined) }
            }
        }
    }
    private var appIsActive: Bool {
        #if os(iOS)
        return UIApplication.shared.applicationState == .active
        #else
        return true
        #endif
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
    func application(_ application: UIApplication, supportedInterfaceOrientationsFor window: UIWindow?) -> UIInterfaceOrientationMask {
        PhoneOrientation.allowed
    }
    func application(_ application: UIApplication, handleEventsForBackgroundURLSession identifier: String,
                     completionHandler: @escaping () -> Void) {
        guard identifier == DownloadQueue.sessionID else { completionHandler(); return }
        DownloadQueue.shared.backgroundCompletion = completionHandler
    }
}
#endif
#endif
