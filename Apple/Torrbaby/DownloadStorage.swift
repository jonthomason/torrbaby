import Foundation

/// Durable ownership and space accounting for device-local media, separate from torrents.
/// All mutations are serialized by the download coordinator on the main actor.
@MainActor
final class DownloadStorage {
    struct Policy: Codable {
        var limitBytes: Int64 = 20_000_000_000
        var freeSpaceBytes: Int64 = 5_000_000_000
        var deleteAfterPlayback = true
        var wifiOnly = true
    }
    enum State: String, Codable { case queued, transferring, paused, ready, removing, removed, failed }
    struct Item: Codable, Identifiable {
        let id: UUID
        let server: String
        let assetID: String
        let titleID: String
        let title: String
        let name: String
        let season: Int?
        let episode: Int?
        let revision: String
        let size: Int64
        let fileExtension: String
        var state: State = .queued
        var received: Int64 = 0
        var pinned = false
        var position: Double = 0
        var duration: Double = 0
        var watched = false
        var playbackFinished = false
        var progressNeedsSync = false
        var progressEventID: UUID?
        var progressRecordedAt: Date?
        var updated = Date()
        var error: String?
    }
    private struct Manifest: Codable {
        var version = 1
        var policy = Policy()
        var items: [Item] = []
    }
    struct StorageError: LocalizedError {
        let message: String
        var errorDescription: String? { message }
    }
    let root: URL
    private var manifest: Manifest
    var items: [Item] { manifest.items }
    var policy: Policy { manifest.policy }
    private var manifestURL: URL { root.appendingPathComponent("downloads.json") }
    private let fm = FileManager.default

    init(root: URL) throws {
        self.root = root.standardizedFileURL
        try FileManager.default.createDirectory(at: self.root, withIntermediateDirectories: true)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: self.root.path)
        let path = self.root.appendingPathComponent("downloads.json")
        if FileManager.default.fileExists(atPath: path.path) {
            manifest = try JSONDecoder().decode(Manifest.self, from: Data(contentsOf: path))
            guard manifest.version == 1 else { throw StorageError(message: "This download library needs a newer app.") }
        } else { manifest = Manifest() }
        guard manifest.policy.limitBytes >= 0, manifest.policy.freeSpaceBytes >= 0,
              manifest.items.allSatisfy({ $0.size > 0 && ["mp4", "m4v", "mov", "mkv", "webm", "avi", "ts", "m2ts"].contains($0.fileExtension) }) else {
            throw StorageError(message: "Download storage records are invalid.")
        }
        var excluded = self.root
        var attributes = URLResourceValues(); attributes.isExcludedFromBackup = true
        try excluded.setResourceValues(attributes)
        try FileManager.default.createDirectory(at: self.root.appendingPathComponent("media"), withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: self.root.appendingPathComponent("partial"), withIntermediateDirectories: true)
        for item in items where item.progressNeedsSync && item.progressEventID == nil {
            try update(item.id) { $0.progressEventID = UUID(); $0.progressRecordedAt = item.updated }
        }
        // A durable removal intent is safe to replay after interruption.
        for item in manifest.items where item.state == .removing { try finishRemoval(item.id) }
        try reconcileFiles()
    }

    /// Runs before reconnecting the background URLSession. Completed delegate
    /// staging files are ours, not URLSession's resumable temporary files.
    private func reconcileFiles() throws {
        let partial = root.appendingPathComponent("partial")
        for file in try fm.contentsOfDirectory(at: partial, includingPropertiesForKeys: [.isRegularFileKey]) {
            guard file.pathExtension == "completed",
                  UUID(uuidString: file.deletingPathExtension().lastPathComponent) != nil,
                  try file.resourceValues(forKeys: [.isRegularFileKey]).isRegularFile == true else { continue }
            try fm.removeItem(at: file)
        }
        for item in items {
            let file = mediaURL(item)
            let attributes = try? fm.attributesOfItem(atPath: file.path)
            if item.state == .ready {
                guard attributes?[.type] as? FileAttributeType == .typeRegular,
                      (attributes?[.size] as? NSNumber)?.int64Value == item.size else {
                    try update(item.id) { $0.state = .failed; $0.received = 0; $0.error = "Downloaded file is missing or incomplete. Download it again." }
                    if attributes != nil { try fm.removeItem(at: file) }
                    continue
                }
            } else if attributes != nil {
                // A crash after move but before durable adoption leaves a file
                // whose revision was never committed. Never present it as ready.
                try fm.removeItem(at: file)
            }
        }
    }

    private func save(_ next: Manifest) throws {
        let encoder = JSONEncoder(); encoder.outputFormatting = [.sortedKeys]
        try encoder.encode(next).write(to: manifestURL, options: [.atomic])
        manifest = next
    }
    private func update(_ id: UUID, _ change: (inout Item) -> Void) throws {
        var next = manifest
        guard let index = next.items.firstIndex(where: { $0.id == id }) else { throw StorageError(message: "Download not found.") }
        change(&next.items[index]); next.items[index].updated = Date()
        try save(next)
    }
    func setPolicy(_ policy: Policy) throws {
        guard policy.limitBytes >= 0, policy.freeSpaceBytes >= 0 else { throw StorageError(message: "Storage limits cannot be negative.") }
        var next = manifest; next.policy = policy
        // Never evict content just because a limit was reduced. The coordinator pauses transfers.
        try save(next)
    }
    func enqueue(server: String, assetID: String, titleID: String, title: String, name: String,
                 season: Int?, episode: Int?, revision: String, size: Int64, fileExtension: String) throws -> Item {
        guard size > 0, !revision.isEmpty, !server.isEmpty, !assetID.isEmpty else { throw StorageError(message: "The file is not ready to download.") }
        if let existing = items.first(where: { $0.server == server && $0.assetID == assetID && $0.state != .removed }) { return existing }
        let ext = fileExtension.lowercased()
        guard ["mp4", "m4v", "mov", "mkv", "webm", "avi", "ts", "m2ts"].contains(ext) else { throw StorageError(message: "Unsupported download format.") }
        let item = Item(id: UUID(), server: server, assetID: assetID, titleID: titleID, title: title, name: name,
                        season: season, episode: episode, revision: revision, size: size, fileExtension: ext)
        var next = manifest; next.items.append(item); try save(next)
        return item
    }
    func mediaURL(_ item: Item) -> URL {
        root.appendingPathComponent("media").appendingPathComponent(item.id.uuidString + "." + item.fileExtension)
    }
    func partialURL(_ id: UUID) -> URL {
        root.appendingPathComponent("partial").appendingPathComponent(id.uuidString + ".partial")
    }
    func resumeURL(_ id: UUID) -> URL {
        root.appendingPathComponent("partial").appendingPathComponent(id.uuidString + ".resume")
    }
    func resumeContextURL(_ id: UUID) -> URL {
        root.appendingPathComponent("partial").appendingPathComponent(id.uuidString + ".context")
    }
    /// Includes posters, partials, resume data and unknown leftovers, not just ready movies.
    func usedBytes() throws -> Int64 {
        guard let files = fm.enumerator(at: root, includingPropertiesForKeys: [.isRegularFileKey, .fileSizeKey, .isSymbolicLinkKey]) else {
            throw StorageError(message: "Cannot read download storage.")
        }
        var total: Int64 = 0
        for case let file as URL in files {
            let info = try file.resourceValues(forKeys: [.isRegularFileKey, .fileSizeKey, .isSymbolicLinkKey])
            if info.isSymbolicLink == true { files.skipDescendants(); continue }
            if info.isRegularFile == true { total += Int64(info.fileSize ?? 0) }
        }
        return total
    }
    func availableBytes() throws -> Int64 {
        let info = try fm.attributesOfFileSystem(forPath: root.path)
        guard let free = info[.systemFreeSize] as? NSNumber else { throw StorageError(message: "Cannot check free storage.") }
        return free.int64Value
    }
    /// Admission reserves the entire remaining file and a small metadata allowance.
    /// Background session temporary bytes may be outside our directory, so account for them too.
    func canStart(_ id: UUID, freeBytes: Int64? = nil) throws -> Bool {
        guard let item = items.first(where: { $0.id == id }), [.queued, .paused, .failed].contains(item.state) else { return false }
        let free = try freeBytes ?? availableBytes()
        let used = try usedBytes()
        let overhead: Int64 = 1_000_000
        var reservations: Int64 = 0
        for active in items where active.state != .removed && active.state != .ready {
            let amount = active.state == .transferring ? active.size : max(0, active.received)
            let (sum, overflow) = reservations.addingReportingOverflow(amount)
            guard !overflow else { return false }
            reservations = sum
        }
        // Reserve the full file conservatively even if a partial exists. This covers
        // resume-data storage and a temporary copy during finalization.
        guard used <= policy.limitBytes, reservations <= policy.limitBytes - used,
              item.size <= policy.limitBytes - used - reservations,
              overhead <= policy.limitBytes - used - reservations - item.size,
              free >= policy.freeSpaceBytes,
              reservations <= free - policy.freeSpaceBytes,
              item.size <= free - policy.freeSpaceBytes - reservations,
              overhead <= free - policy.freeSpaceBytes - reservations - item.size else { return false }
        return true
    }
    func markTransferring(_ id: UUID) throws {
        guard try canStart(id) else { throw StorageError(message: "Download paused: storage limit reached.") }
        try update(id) { $0.state = .transferring; $0.error = nil }
    }
    func pause(_ id: UUID, received: Int64, error: String? = nil) throws {
        try update(id) { item in
            guard item.state != .ready && item.state != .removed && item.state != .removing else { return }
            item.received = min(item.size, max(0, received)); item.state = .paused; item.error = error
        }
    }
    func queue(_ id: UUID) throws {
        try update(id) { if [.paused, .failed].contains($0.state) { $0.state = .queued; $0.error = nil } }
    }
    func pin(_ id: UUID, _ keep: Bool) throws { try update(id) { $0.pinned = keep } }
    func adopt(_ id: UUID, temporary: URL, revision: String) throws {
        guard let item = items.first(where: { $0.id == id }), item.state == .transferring else { throw StorageError(message: "Download was cancelled.") }
        guard item.revision == revision else { throw StorageError(message: "The file changed. Restart this download.") }
        let attrs = try fm.attributesOfItem(atPath: temporary.path)
        guard attrs[.type] as? FileAttributeType == .typeRegular,
              (attrs[.size] as? NSNumber)?.int64Value == item.size else { throw StorageError(message: "The download is incomplete.") }
        let used = try usedBytes()
        let localTemporary = temporary.standardizedFileURL.path.hasPrefix(root.path + "/")
        let additional = localTemporary ? 0 : item.size
        guard used <= policy.limitBytes, additional <= policy.limitBytes - used,
              try availableBytes() >= policy.freeSpaceBytes else {
            throw StorageError(message: "Download paused: storage limit reached.")
        }
        let destination = mediaURL(item)
        guard !fm.fileExists(atPath: destination.path) else { throw StorageError(message: "A downloaded copy already exists.") }
        try fm.moveItem(at: temporary, to: destination)
        // If persistence fails, remove the new copy; never report a non-durable ready item.
        do { try update(id) { $0.state = .ready; $0.received = $0.size; $0.error = nil } }
        catch { try? fm.removeItem(at: destination); throw error }
    }
    func recordPlayback(_ id: UUID, position: Double, duration: Double, reachedEnd: Bool) throws {
        guard position.isFinite, duration.isFinite, duration > 0, position >= 0 else { return }
        try update(id) {
            $0.position = min(position, duration); $0.duration = duration
            $0.watched = reachedEnd || position / duration >= 0.95
            $0.playbackFinished = reachedEnd
            $0.progressNeedsSync = true
            $0.progressEventID = UUID()
            $0.progressRecordedAt = Date()
        }
    }
    func acknowledgeProgress(_ id: UUID, eventID: UUID) throws {
        guard let item = items.first(where: { $0.id == id }), item.progressEventID == eventID else { return }
        try update(id) { $0.progressNeedsSync = false }
    }
    /// Called only after the native player's did-play-to-end event and after releasing its file.
    func removeAfterPlayback(_ id: UUID) throws {
        guard let item = items.first(where: { $0.id == id }), item.playbackFinished,
              !item.pinned, policy.deleteAfterPlayback else { return }
        try remove(id)
    }
    func remove(_ id: UUID) throws {
        try update(id) { $0.state = .removing }
        try finishRemoval(id)
    }
    private func finishRemoval(_ id: UUID) throws {
        guard let item = items.first(where: { $0.id == id }) else { return }
        // Only UUID-derived paths owned by this store can be removed. No server paths.
        for file in [mediaURL(item), partialURL(id), resumeURL(id), resumeContextURL(id)] where fm.fileExists(atPath: file.path) {
            try fm.removeItem(at: file)
        }
        // Retain watch progress until it has synced, independently of the media file.
        try update(id) { $0.state = .removed; $0.received = 0; $0.error = nil }
    }
}
