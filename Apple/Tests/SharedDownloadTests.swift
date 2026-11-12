import Foundation

// Minimal episode value needed by the storage-only test executable.
struct EpisodeNumber: Codable, Hashable { let season: Int; let episode: Int }

/// A download received over AirDrop is ready at once, carries no watch progress, and a
/// second copy of the same title is not added twice.
@main struct SharedDownloadTests {
    @MainActor static func main() throws {
        let fm = FileManager.default
        let root = fm.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? fm.removeItem(at: root) }
        let storage = try DownloadStorage(root: root)
        func received() throws -> URL {
            let file = root.appendingPathComponent("partial").appendingPathComponent(UUID().uuidString + ".mp4")
            try Data(repeating: 7, count: 1234).write(to: file)
            return file
        }
        let order = [EpisodeNumber(season: 1, episode: 1), EpisodeNumber(season: 1, episode: 2)]
        let item = try storage.importReady(server: "shared:http://friend:8765", assetID: "a1", titleID: "t1", title: "Show",
            name: "Show S01E01", season: 1, episode: 1, revision: "r1", fileExtension: "mp4", episodeOrder: order, media: try received())
        assert(item.state == .ready && item.size == 1234 && item.received == 1234)
        assert(item.position == 0 && !item.watched && item.viewing("profile-jon").position == 0)
        assert(fm.fileExists(atPath: storage.mediaURL(item).path))
        assert(storage.items.first?.episodeOrder == order)
        // The same title again keeps the existing copy and discards the new file.
        let again = try received()
        let duplicate = try storage.importReady(server: "shared:http://friend:8765", assetID: "a1", titleID: "t1", title: "Show",
            name: "Show S01E01", season: 1, episode: 1, revision: "r1", fileExtension: "mp4", episodeOrder: nil, media: again)
        assert(duplicate.id == item.id && storage.items.count == 1 && !fm.fileExists(atPath: again.path))
        // It survives a restart as a ready download.
        let reopened = try DownloadStorage(root: root)
        assert(reopened.items.first?.state == .ready)
        // Bad packages are refused.
        var refused = false
        do { _ = try storage.importReady(server: "s", assetID: "a2", titleID: "t", title: "X", name: "X", season: nil, episode: nil,
                                         revision: "r", fileExtension: "exe", episodeOrder: nil, media: try received()) } catch { refused = true }
        assert(refused)
        print("SharedDownloadTests passed")
    }
}
