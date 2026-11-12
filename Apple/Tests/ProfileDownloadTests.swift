import Foundation

// Minimal episode value needed by the storage-only test executable.
struct EpisodeNumber: Codable, Hashable { let season: Int; let episode: Int }

@main struct ProfileDownloadTests {
    @MainActor static func main() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        var storage = try DownloadStorage(root: root)
        let item = try storage.enqueue(server: "http://fixture", assetID: "movie", titleID: "title",
            title: "Movie", name: "Movie.mp4", season: nil, episode: nil, revision: "r1", size: 100, fileExtension: "mp4")
        // Simulate an old manifest: its single progress row belongs only to Jon.
        let manifestURL = root.appendingPathComponent("downloads.json")
        var manifest = try JSONSerialization.jsonObject(with: Data(contentsOf: manifestURL)) as! [String: Any]
        var rows = manifest["items"] as! [[String: Any]]
        rows[0]["position"] = 300; rows[0]["duration"] = 1000; rows[0]["progressNeedsSync"] = true
        rows[0]["progressEventID"] = UUID().uuidString
        rows[0]["progressRecordedAt"] = Date(timeIntervalSince1970: 100).timeIntervalSinceReferenceDate
        manifest["items"] = rows
        try JSONSerialization.data(withJSONObject: manifest).write(to: manifestURL, options: .atomic)
        storage = try DownloadStorage(root: root)
        assert(storage.items[0].viewing("profile-jon").position == 300)
        assert(storage.items[0].viewing("alex").position == 0)
        // Watched only once playback actually reaches the end (b50454c), and only for that person.
        try storage.recordPlayback(item.id, profile: "alex", position: 920, duration: 1000, reachedEnd: false)
        assert(!storage.items[0].viewing("alex").watched)
        try storage.recordPlayback(item.id, profile: "alex", position: 1000, duration: 1000, reachedEnd: true)
        assert(storage.items[0].viewing("alex").watched)
        assert(!storage.items[0].viewing("profile-jon").watched)
        let jonEvent = storage.items[0].viewing("profile-jon").progressEventID!
        try storage.acknowledgeProgress(item.id, profile: "profile-jon", eventID: jonEvent)
        assert(!storage.items[0].viewing("profile-jon").progressNeedsSync)
        assert(storage.items[0].viewing("alex").progressNeedsSync)
        let staleEvent = storage.items[0].viewing("alex").progressEventID!
        try storage.recordPlayback(item.id, profile: "alex", position: 10, duration: 1000, reachedEnd: false)
        try storage.acknowledgeProgress(item.id, profile: "alex", eventID: staleEvent)
        assert(storage.items[0].viewing("alex").progressNeedsSync)
        assert(storage.items[0].viewing("alex").watched)
        try storage.importProgress(item.id, profile: "alex", position: 1, duration: 1000,
            watched: false, recordedAt: Date(timeIntervalSince1970: 100))
        assert(storage.items[0].viewing("alex").position == 10)
        let reopened = try DownloadStorage(root: root)
        assert(reopened.items.count == 1)
        assert(reopened.items[0].viewing("profile-jon").position == 300)
        assert(reopened.items[0].viewing("alex").position == 10)
        assert(reopened.items[0].forProfile("new-person").position == 0)
        assert(reopened.items[0].forProfile("new-person").watched == false)
        print("Profile download isolation, migration, persistence, and event ownership passed")
    }
}
