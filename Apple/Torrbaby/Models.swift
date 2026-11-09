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
    let uploadRatio: Double
    let addedDate: Double
    let activityDate: Double
    let peersConnected: Int
    let peersSendingToUs: Int
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
