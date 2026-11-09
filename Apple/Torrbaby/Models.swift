import Foundation

struct TorrentFile: Codable, Hashable {
    let name: String
    let length: Int64
    let bytesCompleted: Int64
    var complete: Bool { length > 0 && bytesCompleted == length }
    var isVideo: Bool { ["mp4", "m4v", "mov", "mkv", "avi", "webm", "ts", "m2ts", "mpg", "mpeg", "wmv"].contains((name as NSString).pathExtension.lowercased()) }
}

struct Torrent: Codable, Identifiable, Hashable {
    let hashString: String
    let name: String
    let status: Int
    let error: Int
    let errorString: String
    let percentDone: Double
    let downloadDir: String
    let files: [TorrentFile]
    let rateDownload: Int64
    let rateUpload: Int64
    let eta: Int
    var smoothedEta: Int?
    let uploadRatio: Double
    let addedDate: Double
    let activityDate: Double
    let peersConnected: Int
    let peersSendingToUs: Int
    let downloadedEver: Int64?
    let uploadedEver: Int64?
    let isStalled: Bool
    var id: String { hashString }
    var completed: Bool { percentDone >= 1 }
    var needsAttention: Bool { progressIssue != nil }
    var progressIssue: String? {
        guard !completed else { return error != 0 ? errorString : nil }
        if error != 0 { return errorString }
        guard status != 0 else { return nil }
        if isStalled { return "Transmission reports this torrent as stalled" }
        if status == 4 && rateDownload == 0 && Date().timeIntervalSince1970 - max(addedDate, activityDate) > 300 {
            return peersConnected == 0 ? "No peers and no activity for over 5 minutes" : "Peers connected, but no activity for over 5 minutes"
        }
        return nil
    }
    var stateName: String {
        if error != 0 { return "Needs attention" }
        switch status {
        case 0: return "Paused"
        case 1: return "Waiting to verify"
        case 2: return "Verifying"
        case 3: return "Queued"
        case 4: return isStalled ? "Waiting for peers" : "Downloading"
        case 5: return "Queued to seed"
        case 6: return "Seeding"
        default: return "Unknown"
        }
    }
    var displayName: String {
        var value = name
        if files.count == 1 && name == files[0].name { value = (value as NSString).deletingPathExtension }
        return value.replacingOccurrences(of: ".", with: " ").replacingOccurrences(of: "_", with: " ")
    }
    var size: Int64 { files.reduce(0) { $0 + $1.length } }
    var downloaded: Int64 { files.reduce(0) { $0 + $1.bytesCompleted } }
    var etaText: String {
        if completed { return "Complete" }
        if status == 0 { return "Paused" }
        if status != 4 { return stateName }
        if isStalled { return "Waiting for activity" }
        guard let estimate = smoothedEta, estimate >= 0 else { return "Estimating…" }
        if estimate < 60 { return "<1m remaining" }
        if estimate < 3600 { return "~\(max(1, estimate / 60))m remaining" }
        if estimate < 86400 { return "~\(estimate / 3600)h \((estimate % 3600) / 60)m remaining" }
        return "~\(estimate / 86400)d \((estimate % 86400) / 3600)h remaining"
    }
}

struct DaemonSettings: Codable {
    var paused: Bool
    var pollSeconds: Double
    var source: String
    var movies: String
    var tv: String
}
struct Job: Codable, Identifiable {
    let hash: String
    let name: String
    let phase: String
    let error: String?
    let destination: String?
    var id: String { hash }
}
struct DaemonStatus: Codable {
    let version: String
    let uptimeSeconds: Int
    let settings: DaemonSettings
    let lastPoll: Double?
    let lastError: String?
    let pendingScans: Int
    let scanError: String?
    let jobs: [String: Job]
}
struct AuditEvent: Codable, Identifiable {
    let time: Double
    let kind: String
    let hash: String?
    let error: String?
    let destination: String?
    let name: String?
    var id: String { "\(time)-\(kind)-\(hash ?? "")" }
    var title: String { kind.replacingOccurrences(of: "_", with: " ").capitalized }
}
struct LookupResult: Codable {
    let query: String
    let provider: String?
    let candidates: [NameCandidate]
    let note: String
}
struct NameCandidate: Codable, Identifiable {
    let title: String
    let year: String
    let sourceUrl: String?
    let proposedRootName: String?
    let path: String?
    var id: String { (sourceUrl ?? title) + year }
}
struct ClientDevice: Codable, Identifiable {
    let pairedCredential: Int?
    let id: String
    let name: String
    let platform: String
}
struct PlaybackHistory: Codable, Identifiable {
    let id: String
    let position: Double
    let duration: Double
    let watched: Bool
}
struct PlaybackLink: Codable {
    let sessionId: String?
    let resumePosition: Double?
    let path: String
    let name: String
    let expires: Double
}
struct Playback: Identifiable, Codable, Hashable {
    var id = UUID()
    let sessionId: String?
    let resumePosition: Double
    let url: URL
    let name: String
    var localDownloadID: UUID? = nil
    var libraryAssetID: String? = nil
    var libraryTitleID: String? = nil
}

func bytes(_ count: Int64) -> String { ByteCountFormatter.string(fromByteCount: count, countStyle: .file) }
func rate(_ count: Int64) -> String { bytes(count) + "/s" }

struct TransferSample: Codable, Identifiable {
    let time: Double
    let download: Double
    let upload: Double
    var id: Double { time }
    var date: Date { Date(timeIntervalSince1970: time) }
}

struct MediaMetadata: Codable, Identifiable {
    let hash: String
    let status: String
    let selected: MetadataCandidate?
    let candidates: [MetadataCandidate]
    let error: String?
    var id: String { hash }
}
struct MetadataCandidate: Codable {
    let provider: String
    let title: String
    let year: String
    let genres: [String]
    let contentRating: String?
    let durationMinutes: Double?
    let summary: String?
    let thumbnail: String?
    let sourceUrl: String?
    let episodes: [MetadataEpisode]
}
struct MetadataEpisode: Codable {
    let title: String?
    let durationMinutes: Double?
}

/// Shared ordering for Mac, mobile and TV torrent lists.
enum TorrentSort: String, CaseIterable, Identifiable {
    case progress, ratio, added, alphabetic, size
    var id: String { rawValue }
    var label: String {
        switch self {
        case .progress: "Progress"
        case .ratio: "Ratio"
        case .added: "Date added"
        case .alphabetic: "Alphabetical"
        case .size: "Size"
        }
    }
    static func choices(_ category: String) -> [Self] {
        switch category {
        case "Seeding": [.ratio, .added]
        case "Complete": [.alphabetic, .added, .size]
        default: [.progress, .added, .alphabetic]
        }
    }
    func ordered(_ torrents: [Torrent]) -> [Torrent] {
        torrents.sorted { a, b in
            switch self {
            case .progress:
                if a.percentDone != b.percentDone { return a.percentDone > b.percentDone }
            case .ratio:
                if a.uploadRatio != b.uploadRatio { return a.uploadRatio > b.uploadRatio }
            case .added:
                if a.addedDate != b.addedDate { return a.addedDate > b.addedDate }
            case .size:
                if a.size != b.size { return a.size > b.size }
            case .alphabetic: break
            }
            let comparison = a.displayName.localizedStandardCompare(b.displayName)
            return comparison == .orderedSame ? a.id < b.id : comparison == .orderedAscending
        }
    }
}
