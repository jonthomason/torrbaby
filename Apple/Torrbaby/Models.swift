import Foundation

struct TorrentFile: Codable, Hashable {
    let name: String
    let length: Int64
    let bytesCompleted: Int64
    var complete: Bool { length > 0 && bytesCompleted == length }
    var isVideo: Bool { ["mp4", "m4v", "mov", "mkv", "avi", "webm", "ts", "m2ts", "mpg", "mpeg", "wmv"].contains((name as NSString).pathExtension.lowercased()) }
}

struct TorrentFileOperation: Codable, Hashable {
    let id: String
    let action: String
    let phase: String
    let error: String?
    var label: String {
        if let error, !error.isEmpty { return (action == "junk" ? "Junk · " : "Deletion · ") + error }
        return action == "junk" ? "Junk · delete after seeding" : "Deleting files…"
    }
}
struct PendingLibraryOperation: Decodable, Identifiable {
    let id: String
    let action: String
    let phase: String
    let error: String?
    let preview: String
    var name: String {
        guard let data = preview.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let name = object["name"] as? String else { return L10n.text("Library files") }
        return name
    }
    var label: String {
        if phase == "seeding" { return L10n.text("In Junk · delete after seeding") }
        if phase == "moving" || phase == "verifying" { return L10n.text("Moving to Junk…") }
        if phase == "cleanup" { return L10n.text("Finishing cleanup…") }
        return action == "junk" ? "Removing from library…" : "Deleting files…"
    }
}
struct PendingLibraryOperations: Decodable { let operations: [PendingLibraryOperation] }
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
    /// Transmission's bandwidth priority: 1 after "Download first". Absent on older servers.
    let bandwidthPriority: Int?
    var downloadsFirst: Bool { !completed && (bandwidthPriority ?? 0) > 0 }
    var fileOperation: TorrentFileOperation? = nil
    /// Why the server has not filed this finished download into the library.
    var libraryIssue: String? = nil
    var isJunk: Bool { fileOperation?.action == "junk" }
    var id: String { hashString }
    var completed: Bool { percentDone >= 1 }
    var needsAttention: Bool { progressIssue != nil }
    var progressIssue: String? {
        guard !completed else { return error != 0 ? errorString : libraryIssue.map { L10n.text($0) } }
        if error != 0 { return errorString }
        guard status != 0 else { return nil }
        if isStalled { return L10n.text("Transmission reports this torrent as stalled") }
        if status == 4 && rateDownload == 0 && Date().timeIntervalSince1970 - max(addedDate, activityDate) > 300 {
            return L10n.text(peersConnected == 0 ? "No peers and no activity for over 5 minutes" : "Peers connected, but no activity for over 5 minutes")
        }
        return nil
    }
    var stateName: String {
        if let operation = fileOperation { return operation.label }
        if error != 0 { return L10n.text("Needs attention") }
        switch status {
        case 0: return L10n.text("Paused")
        case 1: return L10n.text("Waiting to verify")
        case 2: return L10n.text("Verifying")
        case 3: return L10n.text("Queued")
        case 4: return L10n.text(isStalled ? "Waiting for peers" : "Downloading")
        case 5: return L10n.text("Queued to seed")
        case 6: return L10n.text("Seeding")
        default: return L10n.text("Status unavailable")
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
        if completed { return L10n.text("Complete") }
        if status == 0 { return L10n.text("Paused") }
        if status != 4 { return stateName }
        if isStalled { return L10n.text("Waiting for activity") }
        guard let estimate = smoothedEta, estimate >= 0 else { return L10n.text("Estimating…") }
        if estimate < 60 { return L10n.text("<1m remaining") }
        if estimate < 3600 { return L10n.format("~%lldm remaining", max(1, estimate / 60)) }
        if estimate < 86400 { return L10n.format("~%lldh %lldm remaining", estimate / 3600, (estimate % 3600) / 60) }
        return L10n.format("~%lldd %lldh remaining", estimate / 86400, (estimate % 86400) / 3600)
    }
}

struct DaemonSettings: Codable {
    var backgroundSubtitles: Bool?
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
/// A processing problem in Needs attention: an error, or a warning that an optional stage
/// (credits, subtitles) was skipped. Dismissing marks it as seen on the server.
struct AttentionItem: Decodable, Identifiable {
    let id: Int
    let assetId: String
    let title: String
    let stage: String
    let severity: String
    let summary: String
    let error: String
    let occurrences: Int
    let lastSeen: Double
}
struct AttentionList: Decodable { let items: [AttentionItem] }

struct SubtitleQueueItem: Codable, Identifiable {
    /// What the file is, as the library names it (older servers send only `title`).
    let show: String?
    let season: Int?
    let episode: Int?
    let episodeName: String?
    /// "Season 3 Ep 12 · The Bakra", or a special's own name; nil for a movie.
    var episodeLine: String? {
        if let season, let episode {
            let number = L10n.format("Season %d Ep %d", season, episode)
            if let name = episodeName, !name.isEmpty { return number + " · " + name }
            return number
        }
        return episodeName.flatMap { $0.isEmpty ? nil : $0 }
    }
    let stage: String?
    /// While subtitles are made from the title itself: "picture" when they are read from the
    /// subtitles printed in the picture, "speech" when the audio is transcribed.
    let source: String?
    let subtitlesState: String?
    let inventoryState: String?
    let creditsState: String?
    /// One file's work, step by step: languages, credits, subtitles (made from text tracks,
    /// the picture or the audio), translation. Each says whether it is done, running
    /// (with progress), waiting, not needed or failed.
    struct Step: Identifiable {
        enum Status { case done, running, waiting, skipped, failed }
        let id: String
        let label: String
        let status: Status
        var progress: String? = nil
        var fraction: Double? = nil
    }
    var steps: [Step] {
        let running = state == "running"
        func settled(_ value: String?) -> Step.Status? {
            switch value {
            case "ready": return .done
            case "not_applicable", "skipped": return .skipped
            case "incomplete": return .failed
            default: return nil
            }
        }
        var result: [Step] = []
        let languages = settled(inventoryState) ?? (running && stage == "inventory" ? .running : state == "failed" && stage == "inventory" ? .failed : .waiting)
        result.append(Step(id: "languages", label: "Languages", status: languages))
        // In the server's order: credits are found right after the languages, ahead of the
        // subtitles, which take far longer and run a few files at a time.
        let credits = settled(creditsState) ?? (running && stage == "credits" ? .running : state == "failed" && stage == "credits" ? .failed : .waiting)
        result.append(Step(id: "credits", label: "Credits", status: credits))
        let counted = totalCues > 0 ? progressText : nil
        let fraction = totalCues > 0 ? Double(completedCues) / Double(totalCues) : nil
        let making = ["subtitles", "ocr", "transcription"].contains(stage ?? "")
        if let status = settled(subtitlesState) {
            result.append(Step(id: "subtitles", label: "Subtitles", status: status))
            result.append(Step(id: "translation", label: "Translation", status: status))
        } else if running && making {
            result.append(Step(id: "subtitles", label: stageLabel, status: .running, progress: counted, fraction: fraction))
            result.append(Step(id: "translation", label: "Translation", status: .waiting))
        } else if running && stage == "translation" {
            result.append(Step(id: "subtitles", label: "Subtitles", status: .done))
            result.append(Step(id: "translation", label: stageLabel, status: .running, progress: counted, fraction: fraction))
        } else {
            let failed = state == "failed" && (making || stage == "translation")
            result.append(Step(id: "subtitles", label: "Subtitles", status: failed ? .failed : .waiting))
            result.append(Step(id: "translation", label: "Translation", status: .waiting))
        }
        return result
    }
    /// How the subtitle text was obtained: "text", "ocr" or "speech".
    let method: String?
    var stageLabel: String {
        switch stage {
        case "inventory": return state == "running" ? "Checking languages…" : "Waiting to check languages"
        case "credits": return "Detecting credits"
        case "complete": return "All processing complete"
        case "transcription":
            switch source {
            case "picture": return "Reading subtitles from the picture"
            case "speech": return "Transcribing audio"
            default: return "Preparing subtitles"
            }
        case "ocr": return "Reading image subtitles (OCR)"
        case "translation":
            switch method {
            case "ocr": return "Translating OCR subtitles"
            case "speech": return "Translating audio transcript"
            default: return "Translating text subtitles"
            }
        default:
            switch method {
            case "ocr": return "Reading image subtitles (OCR)"
            case "speech": return "Preparing audio transcript"
            case "text": return "Extracting text subtitles"
            // Not started: how its subtitles will be made is decided when it starts.
            default: return state == "running" ? "Preparing subtitles" : "Waiting for subtitles"
            }
        }
    }
    /// Transcription reports seconds of audio; other stages report cues.
    var progressText: String {
        guard stage == "transcription" else { return "\(completedCues) / \(totalCues)" }
        func clock(_ seconds: Int) -> String {
            seconds >= 3600 ? String(format: "%d:%02d:%02d", seconds / 3600, seconds / 60 % 60, seconds % 60)
                : String(format: "%d:%02d", seconds / 60, seconds % 60)
        }
        return "\(clock(completedCues)) / \(clock(totalCues))"
    }
    let id: String
    let assetId: String?
    let title: String
    let state: String
    let target: String?
    let error: String?
    let completedCues: Int
    let totalCues: Int
}
/// Short subtitle track names: "English Text", "English Images", "English SDH",
/// "English Voice Transcribed", "Chinese Translated". `kind` and `sdh` come from
/// the server; SDH applies to original text or image tracks, not to translations.
enum SubtitleTrackName {
    static func key(language: String?, kind: String?, sdh: Bool = false) -> String? {
        let code = (language ?? "").lowercased()
        let name = code == "en" || code == "eng" || code.hasPrefix("en-") ? "English"
            : code == "zh" || code == "zho" || code == "chi" || code.hasPrefix("zh-") ? "Chinese" : nil
        guard let name else { return nil }
        if sdh && ["text", "images", nil].contains(kind) { return "\(name) SDH" }
        let kinds = ["images": "Images", "speech": "Voice Transcribed", "translated": "Translated"]
        return "\(name) \(kinds[kind ?? ""] ?? "Text")"
    }
    static func text(language: String?, kind: String?, sdh: Bool = false) -> String? {
        key(language: language, kind: kind, sdh: sdh).map { L10n.text($0) }
    }
}
struct SubtitlePreparation: Codable {
    let queue: [SubtitleQueueItem]?
    struct Current: Codable { let assetId: String; let language: String; let completedCues: Int; let totalCues: Int }
    let scanned: Int
    let translated: Int
    let failed: Int
    let waitingForIdle: Bool
    let current: Current?
}
struct CreditsAnalysis: Codable {
    struct Current: Codable { let assetId: String; let title: String }
    let scanned: Int
    let detected: Int
    let failed: Int
    let current: Current?
}
/// The server's own name and id, fixed when it was first set up.
struct ServerIdentity: Codable {
    struct Routes: Codable { let home: String?; let away: String? }
    let name: String?; let id: String?; var routes: Routes? = nil
}
/// The online check's answer.
struct ServerPing: Codable { var version: String? = nil; var server: ServerIdentity? = nil }
struct DaemonStatus: Codable {
    var server: ServerIdentity? = nil
    let creditsAnalysis: CreditsAnalysis?
    let subtitlePreparation: SubtitlePreparation?
    let version: String
    let uptimeSeconds: Int
    let settings: DaemonSettings
    let lastPoll: Double?
    let lastError: String?
    let pendingScans: Int
    let scanError: String?
    let jobs: [String: Job]
    /// The drives behind the library roots; absent on servers before 1.0.
    var storage: MediaStorage? = nil
}
/// Free space on the server's media drives.
struct MediaStorage: Codable {
    struct Volume: Codable { let name: String; let totalBytes: Int64; let freeBytes: Int64; let roles: [String]; let available: Bool }
    let freeBytes: Int64
    let totalBytes: Int64
    let volumes: [Volume]
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

/// Read the installed bundle so every platform reports the actual release.
enum AppRelease {
    static var description: String {
        let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "—"
        let build = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "—"
        return "Version \(version) · Build \(build)"
    }
}

struct SubtitlePreferences: Codable, Equatable {
    // Initial choice for a new device; never inherited from a household/profile.
    var enabled = true
    var language = "en"
    var font = "default"
    var sizePercent = 100
    var textOpacity = 100
    var backgroundOpacity = 0
    var body: [String: Any] {
        ["enabled": enabled, "language": language, "font": font, "size_percent": sizePercent,
         "text_opacity": textOpacity, "background_opacity": backgroundOpacity]
    }
}
struct SubtitlePreferenceResponse: Decodable {
    let defaults: SubtitlePreferences
    let effective: SubtitlePreferences
    let overridden: Bool
}
