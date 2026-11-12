import Foundation

/// Durable ownership and space accounting for device-local media, separate from torrents.
/// All mutations are serialized by the download coordinator on the main actor.
@MainActor
final class DownloadStorage {
    struct Policy: Codable {
        var limitBytes: Int64 = 20_000_000_000
        var freeSpaceBytes: Int64 = 0
        var useAvailableSpace = true
        var deleteAfterPlayback = true
        /// The Mac's standing choice for hotspots. iPhone and iPad use `meteredUntil` instead.
        var wifiOnly = true
        /// On iPhone and iPad, cellular and hotspot downloads are allowed only until this time
        /// (seconds since 1970), at most three hours after the user allows them.
        var meteredUntil: Double? = nil
        init() {}
        private enum CodingKeys: String, CodingKey { case limitBytes, freeSpaceBytes, useAvailableSpace, deleteAfterPlayback, wifiOnly, meteredUntil }
        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            limitBytes = try c.decode(Int64.self, forKey: .limitBytes)
            freeSpaceBytes = try c.decode(Int64.self, forKey: .freeSpaceBytes)
            deleteAfterPlayback = try c.decode(Bool.self, forKey: .deleteAfterPlayback)
            wifiOnly = try c.decode(Bool.self, forKey: .wifiOnly)
            meteredUntil = try c.decodeIfPresent(Double.self, forKey: .meteredUntil)
            if let available = try c.decodeIfPresent(Bool.self, forKey: .useAvailableSpace) {
                useAvailableSpace = available
            } else {
                // Upgrade the old defaults; preserve explicitly customized budgets.
                useAvailableSpace = limitBytes == 20_000_000_000 && freeSpaceBytes == 5_000_000_000
                if useAvailableSpace { freeSpaceBytes = 0 }
            }
        }
        func permits(used: Int64, additional: Int64 = 0) -> Bool {
            guard used >= 0, additional >= 0 else { return false }
            return useAvailableSpace || (used <= limitBytes && additional <= limitBytes - used)
        }

    }
    enum State: String, Codable { case queued, transferring, paused, ready, removing, removed, failed }
    struct Viewing: Codable {
        var position: Double = 0
        var duration: Double = 0
        var watched = false
        var playbackFinished = false
        var progressNeedsSync = false
        var progressEventID: UUID?
        var progressRecordedAt: Date?
    }
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
        var virtualJob: String? = nil
        var episodeOrder: [EpisodeNumber]?
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
        var profileViewing: [String: Viewing]?
        func viewing(_ profile: String) -> Viewing {
            if let value = profileViewing?[profile] { return value }
            if profile == "profile-jon" {
                return Viewing(position: position, duration: duration, watched: watched,
                    playbackFinished: playbackFinished, progressNeedsSync: progressNeedsSync,
                    progressEventID: progressEventID, progressRecordedAt: progressRecordedAt)
            }
            return Viewing()
        }
        var allViewing: [String: Viewing] {
            var result = profileViewing ?? [:]
            if result["profile-jon"] == nil { result["profile-jon"] = viewing("profile-jon") }
            return result
        }
        func forProfile(_ profile: String) -> Item {
            var copy = self
            let value = viewing(profile)
            copy.position = value.position; copy.duration = value.duration; copy.watched = value.watched
            copy.playbackFinished = value.playbackFinished; copy.progressNeedsSync = value.progressNeedsSync
            copy.progressEventID = value.progressEventID; copy.progressRecordedAt = value.progressRecordedAt
            return copy
        }
        var posterKey: String?
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
        try FileManager.default.createDirectory(at: self.root.appendingPathComponent("artwork"), withIntermediateDirectories: true)
        for item in items where item.progressNeedsSync && item.progressEventID == nil {
            try update(item.id) { $0.progressEventID = UUID(); $0.progressRecordedAt = item.updated }
        }
        // A durable removal intent is safe to replay after interruption.
        for item in manifest.items where item.state == .removing { try finishRemoval(item.id) }
        try reconcileFiles()
        try pruneArtwork()
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
    func rememberEpisodeOrder(_ id: UUID, episodes: [EpisodeNumber]) throws {
        guard !episodes.isEmpty, let item = items.first(where: { $0.id == id }), item.episodeOrder != episodes else { return }
        try update(id) { $0.episodeOrder = episodes }
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
                 season: Int?, episode: Int?, revision: String, size: Int64, fileExtension: String, virtualJob: String? = nil) throws -> Item {
        guard size > 0, !revision.isEmpty, !server.isEmpty, !assetID.isEmpty else { throw StorageError(message: "The file is not ready to download.") }
        if let existing = items.first(where: { $0.server == server && $0.assetID == assetID && $0.state != .removed }) { return existing }
        let ext = fileExtension.lowercased()
        guard ["mp4", "m4v", "mov", "mkv", "webm", "avi", "ts", "m2ts"].contains(ext) else { throw StorageError(message: "Unsupported download format.") }
        let item = Item(id: UUID(), server: server, assetID: assetID, titleID: titleID, title: title, name: name,
                        season: season, episode: episode, revision: revision, size: size, fileExtension: ext, virtualJob: virtualJob)
        var next = manifest; next.items.append(item); try save(next)
        return item
    }
    func mediaURL(_ item: Item) -> URL {
        root.appendingPathComponent("media").appendingPathComponent(item.id.uuidString + "." + item.fileExtension)
    }
    func posterURL(_ key: String) -> URL? {
        guard key.count == 64, key.utf8.allSatisfy({ (48...57).contains($0) || (97...102).contains($0) }) else { return nil }
        return root.appendingPathComponent("artwork").appendingPathComponent(key + ".jpg")
    }
    func savePoster(_ id: UUID, key: String, data: Data) throws {
        guard !data.isEmpty, data.count <= 524_288, let destination = posterURL(key),
              let item = items.first(where: { $0.id == id }), item.state != .removed && item.state != .removing else { return }
        let existed = fm.fileExists(atPath: destination.path)
        if !existed {
            let used = try accountedBytes()
            let free = try availableBytes()
            guard policy.permits(used: used, additional: Int64(data.count)),
                  free >= policy.freeSpaceBytes, Int64(data.count) <= free - policy.freeSpaceBytes else { return }
            try data.write(to: destination, options: .atomic)
        }
        do { try update(id) { $0.posterKey = key } }
        catch { if !existed { try? fm.removeItem(at: destination) }; throw error }
    }
    private func pruneArtwork() throws {
        let retained = Set(items.filter { $0.state != .removed }.compactMap(\.posterKey))
        let folder = root.appendingPathComponent("artwork")
        for file in try fm.contentsOfDirectory(at: folder, includingPropertiesForKeys: nil) {
            let key = file.deletingPathExtension().lastPathComponent
            guard file.pathExtension == "jpg", posterURL(key) != nil, !retained.contains(key) else { continue }
            try fm.removeItem(at: file)
        }
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
    /// Logical usage includes URLSession's partial bytes outside our owned folder.
    /// A tracked partial already inside the folder must not be counted twice.
    func accountedBytes(received: [UUID: Int64] = [:], excluding: UUID? = nil) throws -> Int64 {
        var total = try usedBytes()
        for item in items where item.id != excluding && [.queued, .transferring, .paused, .failed].contains(item.state) {
            let known = min(item.size, max(0, received[item.id] ?? item.received))
            let additional = max(0, known - ownedPartialBytes(item.id))
            let (sum, overflow) = total.addingReportingOverflow(additional)
            guard !overflow else { throw StorageError(message: "Download storage total is too large.") }
            total = sum
        }
        return total
    }
    private func ownedPartialBytes(_ id: UUID) -> Int64 {
        let attrs = try? fm.attributesOfItem(atPath: partialURL(id).path)
        guard attrs?[.type] as? FileAttributeType == .typeRegular else { return 0 }
        return (attrs?[.size] as? NSNumber)?.int64Value ?? 0
    }
    /// Reserve final logical sizes once, rather than partial + full size for a resume.
    /// Free-space admission remains conservative enough for a full-file restart.
    func canStart(_ id: UUID, freeBytes: Int64? = nil) throws -> Bool {
        guard let item = items.first(where: { $0.id == id }), [.queued, .paused, .failed].contains(item.state) else { return false }
        let free = try freeBytes ?? availableBytes()
        var projected = try accountedBytes(excluding: id)
        var diskNeeded: Int64 = 1_000_000
        for active in items where active.id != id && active.state == .transferring {
            let remaining = max(0, active.size - active.received)
            let (next, overflow) = projected.addingReportingOverflow(remaining)
            let (disk, diskOverflow) = diskNeeded.addingReportingOverflow(remaining)
            guard !overflow && !diskOverflow else { return false }
            projected = next; diskNeeded = disk
        }
        let additional = max(0, item.size - ownedPartialBytes(id))
        let (withItem, overflow) = projected.addingReportingOverflow(additional)
        let (withOverhead, overheadOverflow) = withItem.addingReportingOverflow(1_000_000)
        let (disk, diskOverflow) = diskNeeded.addingReportingOverflow(item.size)
        guard !overflow && !overheadOverflow && !diskOverflow else { return false }
        return policy.permits(used: withOverhead) && free >= policy.freeSpaceBytes
            && disk <= free - policy.freeSpaceBytes
    }
    func markTransferring(_ id: UUID) throws {
        guard try canStart(id) else { throw StorageError(message: "Download paused: not enough available storage.") }
        try update(id) { $0.state = .transferring; $0.error = nil }
    }
    func resetTransferBytes(_ id: UUID) throws {
        try update(id) { if $0.state == .transferring { $0.received = 0 } }
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
        let used = try accountedBytes(excluding: id)
        let localTemporary = temporary.standardizedFileURL.path.hasPrefix(root.path + "/")
        let additional = localTemporary ? 0 : item.size
        guard policy.permits(used: used, additional: additional),
              try availableBytes() >= policy.freeSpaceBytes else {
            throw StorageError(message: "Download paused: not enough available storage.")
        }
        let destination = mediaURL(item)
        guard !fm.fileExists(atPath: destination.path) else { throw StorageError(message: "A downloaded copy already exists.") }
        try fm.moveItem(at: temporary, to: destination)
        // If persistence fails, remove the new copy; never report a non-durable ready item.
        do { try update(id) { $0.state = .ready; $0.received = $0.size; $0.error = nil } }
        catch { try? fm.removeItem(at: destination); throw error }
    }
    func recordPlayback(_ id: UUID, profile: String = "profile-jon", position: Double, duration: Double, reachedEnd: Bool, watchedEligible: Bool = true) throws {
        guard position.isFinite, duration.isFinite, duration > 0, position >= 0 else { return }
        try update(id) { item in
            var value = item.viewing(profile)
            value.position = min(position, duration); value.duration = duration
            value.watched = value.watched || reachedEnd || (watchedEligible && position >= duration)
            value.playbackFinished = reachedEnd
            value.progressNeedsSync = true
            value.progressEventID = UUID()
            value.progressRecordedAt = Date()
            item.profileViewing = item.allViewing
            item.profileViewing?[profile] = value
        }
    }
    func acknowledgeProgress(_ id: UUID, profile: String = "profile-jon", eventID: UUID) throws {
        guard let item = items.first(where: { $0.id == id }), item.viewing(profile).progressEventID == eventID else { return }
        try update(id) { item in
            var value = item.viewing(profile)
            value.progressNeedsSync = false
            item.profileViewing = item.allViewing
            item.profileViewing?[profile] = value
        }
    }
    func importProgress(_ id: UUID, profile: String = "profile-jon", position: Double, duration: Double, watched: Bool, recordedAt: Date) throws {
        guard position.isFinite, duration.isFinite, duration >= 0, position >= 0,
              position <= duration + 2, recordedAt.timeIntervalSince1970.isFinite,
              recordedAt.timeIntervalSince1970 > 0,
              let item = items.first(where: { $0.id == id }),
              recordedAt > (item.viewing(profile).progressRecordedAt ?? .distantPast) else { return }
        try update(id) { item in
            var value = item.viewing(profile)
            value.position = min(position, duration); value.duration = duration; value.watched = watched
            value.progressRecordedAt = recordedAt
            value.progressNeedsSync = false; value.progressEventID = nil
            value.playbackFinished = false
            item.profileViewing = item.allViewing
            item.profileViewing?[profile] = value
        }
    }
    /// Called only after the native player's did-play-to-end event and after releasing its file.
    func removeAfterPlayback(_ id: UUID, profile: String = "profile-jon") throws {
        guard let item = items.first(where: { $0.id == id }), item.viewing(profile).playbackFinished, item.allViewing.count <= 1,
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
        try pruneArtwork()
    }
}
