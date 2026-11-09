#if !os(tvOS)
import Foundation
import CryptoKit
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
    private var session: URLSession!
    nonisolated private let staging: URL
    var backgroundCompletion: (() -> Void)? { didSet { completeBackgroundEvents() } }
    private var finishing = 0
    private var backgroundEventsFinished = false

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
            }
            for item in items where item.state == .transferring && tasks[item.id] == nil {
                try? storage?.pause(item.id, received: item.received, error: "Transfer interrupted. Tap resume.")
            }
            restored = true; reload(); pump()
        }
    }
    func attach(_ store: Store) { self.store = store; pump() }
    private func reload() {
        guard let storage else { return }
        items = storage.items; policy = storage.policy
        do { used = try storage.usedBytes() } catch { self.error = error.localizedDescription }
    }
    func setPolicy(_ next: DownloadStorage.Policy) {
        do {
            try storage?.setPolicy(next); reload()
            // Apply reduced limits and network policy to active requests by pausing them.
            for id in Array(tasks.keys) { pause(id) }
        } catch { self.error = error.localizedDescription }
    }
    func enqueue(_ asset: LibraryAsset, title: LibraryTitle) async {
        guard let store, let storage else { return }
        do {
            struct Info: Decodable { let name: String; let size: Int64; let etag: String }
            let info: Info = try await store.request("GET", "/v1/library/assets/\(asset.id)/download-info")
            _ = try storage.enqueue(server: store.server, assetID: asset.id, titleID: title.id,
                                    title: title.title, name: asset.name ?? info.name,
                                    season: asset.season, episode: asset.episode, revision: info.etag,
                                    size: info.size, fileExtension: URL(fileURLWithPath: info.name).pathExtension)
            reload(); pump()
        } catch { self.error = error.localizedDescription }
    }
    private func pump() {
        guard restored, tasks.isEmpty, let store, store.configured, let storage else { return }
        guard let item = items.first(where: { $0.state == .queued && $0.server == store.server }) else { return }
        do {
            try storage.markTransferring(item.id)
            var request = URLRequest(url: try store.endpoint("/v1/library/assets/\(item.assetID)/download"))
            request.setValue("Bearer " + store.token, forHTTPHeaderField: "Authorization")
            request.setValue(item.revision, forHTTPHeaderField: "If-Match")
            request.allowsCellularAccess = !policy.wifiOnly
            request.allowsExpensiveNetworkAccess = !policy.wifiOnly
            request.timeoutInterval = 120
            let context = Data(SHA256.hash(data: Data("\(store.server)|\(store.token)|\(policy.wifiOnly)".utf8)))
            let task: URLSessionDownloadTask
            if (try? Data(contentsOf: storage.resumeContextURL(item.id))) == context,
               let data = try? Data(contentsOf: storage.resumeURL(item.id)) {
                task = session.downloadTask(withResumeData: data)
            } else {
                try? FileManager.default.removeItem(at: storage.resumeURL(item.id))
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
    func pause(_ id: UUID) {
        guard let storage, let item = items.first(where: { $0.id == id }) else { return }
        do { try storage.pause(id, received: received[id] ?? item.received) }
        catch { self.error = error.localizedDescription; return }
        reload()
        guard let task = tasks[id] else { return }
        task.cancel { [weak self] data in
            DispatchQueue.main.async {
                guard let self, let storage = self.storage else { return }
                if let data, self.items.first(where: { $0.id == id })?.state == .paused {
                    do { try data.write(to: storage.resumeURL(id), options: .atomic) }
                    catch { self.error = error.localizedDescription }
                }
                if self.tasks[id] === task { self.tasks.removeValue(forKey: id) }
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
    func isEpisode(_ playback: Playback) -> Bool {
        if let id = playback.localDownloadID { return items.first(where: { $0.id == id })?.episode != nil }
        return store?.library?.titles.first(where: { $0.id == playback.libraryTitleID })?.assets.first(where: { $0.id == playback.libraryAssetID })?.episode != nil
    }
    func nextPlayback(after currentPlayback: Playback) async -> Playback? {
        let local = items.first { $0.id == currentPlayback.localDownloadID }
        let titleID = local?.titleID ?? currentPlayback.libraryTitleID
        let title = store?.library?.titles.first { $0.id == titleID }
        let currentAsset = title?.assets.first { $0.id == currentPlayback.libraryAssetID }
        guard let season = local?.season ?? currentAsset?.season,
              let episode = local?.episode ?? currentAsset?.episode,
              let server = local?.server ?? store?.server else { return nil }
        if let next = items.first(where: { $0.server == server && $0.titleID == titleID && $0.season == season && $0.episode == episode + 1 && $0.state == .ready }) {
            return playback(next.id)
        }
        guard let store, store.server == server, store.connected, let title,
              let asset = title.assets.first(where: { $0.role != "extra" && $0.season == season && $0.episode == episode + 1 && $0.complete }) else { return nil }
        do {
            try await store.registerClient()
            let link: PlaybackLink = try await store.request("POST", "/v1/library/assets/\(asset.id)/play", body: ["client_id": store.clientId])
            return Playback(sessionId: link.sessionId, resumePosition: link.resumePosition ?? 0,
                            url: try store.endpoint(link.path), name: asset.name ?? title.title,
                            libraryAssetID: asset.id, libraryTitleID: title.id)
        } catch { return nil }
    }
    @discardableResult
    func report(_ id: UUID, position: Double, duration: Double, reachedEnd: Bool) -> Bool {
        guard let storage, position.isFinite, duration.isFinite, duration > 0, position >= 0 else { return false }
        do {
            try storage.recordPlayback(id, position: position, duration: duration, reachedEnd: reachedEnd)
            reload(); return true
        } catch { self.error = error.localizedDescription; return false }
    }
    func finished(_ id: UUID) {
        do { try storage?.removeAfterPlayback(id); reload(); pump() }
        catch { self.error = error.localizedDescription }
    }
    nonisolated func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask,
                                didWriteData bytesWritten: Int64, totalBytesWritten: Int64, totalBytesExpectedToWrite: Int64) {
        guard let id = downloadTask.taskDescription.flatMap(UUID.init(uuidString:)) else { return }
        DispatchQueue.main.async {
            guard self.tasks[id] === downloadTask else { return }
            self.received[id] = totalBytesWritten
            if let storage = self.storage, let free = try? storage.availableBytes(), free < self.policy.freeSpaceBytes {
                self.pause(id)
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
                guard let response, (200...299).contains(response.statusCode) else {
                    throw APIError(message: response?.statusCode == 412 ? "The file changed. Remove this download and add it again." : "Download failed. Reconnect and try again.")
                }
                try self.storage?.adopt(id, temporary: file, revision: response.value(forHTTPHeaderField: "ETag") ?? "")
                if let storage = self.storage { try? FileManager.default.removeItem(at: storage.resumeURL(id)) }
            } catch {
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
            if let storage = self.storage {
                if let resume { try? resume.write(to: storage.resumeURL(id), options: .atomic) }
                else { try? FileManager.default.removeItem(at: storage.resumeURL(id)) }
                try? storage.pause(id, received: self.received[id] ?? 0, error: "Download interrupted. Tap resume.")
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
