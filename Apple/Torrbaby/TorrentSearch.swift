import SwiftUI

struct TorrentSearchSource: Decodable {
    let provider: String
    let id: String
    let url: String
    var label: String { Self.label(provider) }
    static func label(_ provider: String) -> String { ["tpb":"The Pirate Bay", "csv":"Torrent CSV", "nyaa":"Nyaa", "quark": L10n.text("Quark Drive")][provider] ?? provider }
}
struct TorrentReleaseHints: Decodable {
    let year: Int?
    let season: Int?
    var seasonEnd: Int? = nil
    let episode: Int?
    let episodeEnd: Int?
    let episodeCount: Int?
    let resolution: String?
    let videoCodec: String?
    let releaseSource: String?
    let audio: [String]
    let audioKind: String?
    let audioChannels: String?
    let languages: [String]
    /// The release's audio in plain words; the raw tags when the server could not classify them.
    var audioLabel: String {
        switch audioKind {
        case "lossless_atmos": return L10n.text("Lossless Atmos")
        case "dtsx": return "DTS:X"
        case "atmos": return L10n.text("Dolby Atmos")
        case "lossless": return L10n.text("Lossless") + (audioChannels.map { " " + $0 } ?? "")
        case "surround": return L10n.text("Surround") + (audioChannels.map { " " + $0 } ?? "")
        case "stereo": return L10n.text("Stereo")
        case "mono": return L10n.text("Mono")
        default: return audio.joined(separator: " · ")
        }
    }
    var summary: String { ([resolution, videoCodec, releaseSource].compactMap { $0 } + audio + languages).joined(separator: " · ") }
}
/// The tracks the server read from a copy's first episode, without downloading it.
struct CloudMeasured: Decodable {
    struct Track: Decodable {
        var format: String? = nil; var layout: String? = nil; var language: String? = nil; var channels: Int? = nil
        /// "Dolby Atmos 5.1", "AAC Stereo".
        var label: String {
            let shape = layout == "stereo" ? L10n.text("Stereo") : layout == "mono" ? L10n.text("Mono") : layout ?? ""
            return [format ?? "", shape].filter { !$0.isEmpty }.joined(separator: " ")
        }
    }
    struct Video: Decodable { var resolution: String? = nil; var width: Int? = nil; var height: Int? = nil; var codec: String? = nil; var hdr: String? = nil }
    var audio: [Track]? = nil
    var video: Video? = nil
    var subtitles: [String]? = nil
    /// The fullest sound track, which is the one a player picks.
    var bestTrack: Track? { audio?.max { ($0.channels ?? 0, $0.format?.contains("Atmos") == true ? 1 : 0) < ($1.channels ?? 0, $1.format?.contains("Atmos") == true ? 1 : 0) } }
    static func language(_ code: String) -> String { code == "und" ? L10n.text("Unknown") : L10n.language(code) }
}
/// One copy of a series inside a cloud share, as the server measured it: sharers often
/// bundle the same episodes in several qualities, and each is its own search result.
struct CloudCopy: Decodable {
    struct Picture: Decodable { let resolution: String; let count: Int }
    var key: String? = nil
    var folder: String? = nil
    var resolution: String? = nil
    /// Every picture size in the set with how many videos have it, most common first.
    var pictures: [Picture]? = nil
    var fps: Int? = nil
    var hdr: String? = nil
    var codec: String? = nil
    /// "atmos", "5.1", "7.1" or "stereo", as the share's names state it; the drive does not measure sound.
    var audio: String? = nil
    var season: Int? = nil
    var episodes: [Int]? = nil
    /// Videos with no episode number: trailers and extras.
    var extras: Int? = nil
    var videoCount: Int? = nil
    var totalBytes: Int64? = nil
    var minutes: Int? = nil
    var mbps: Double? = nil
    /// The season's episodes aired so far and in all, when the series is known.
    var aired: Int? = nil
    var total: Int? = nil
    var missing: [Int]? = nil
    var complete: Bool? = nil
    /// Numbered past the series' last episode: another cut, whose numbers will not line up.
    var beyond: Bool? = nil
    /// The series' own title.
    var show: String? = nil
    /// Its tracks, once the server has read them.
    var measured: CloudMeasured? = nil

    /// "E01–E11 E13–E34": runs of consecutive episodes.
    static func ranges(_ numbers: [Int], limit: Int = 5) -> String {
        var runs: [(Int, Int)] = []
        for number in numbers.sorted() {
            if let last = runs.last, number <= last.1 + 1 { runs[runs.count - 1].1 = max(last.1, number) } else { runs.append((number, number)) }
        }
        let text = runs.prefix(limit).map { $0.0 == $0.1 ? String(format: "E%02d", $0.0) : String(format: "E%02d–E%02d", $0.0, $0.1) }
        return (text + (runs.count > limit ? ["…"] : [])).joined(separator: " ")
    }
    /// Which episodes the copy holds, against the series when it is known:
    /// "Complete · 34 episodes", "33 of 34 episodes · Missing E29", "12 episodes · E01–E12".
    var episodesLine: String {
        let held = episodes ?? []
        guard !held.isEmpty else { return (videoCount ?? 0) == 1 ? L10n.text("1 video") : L10n.format("%d videos", videoCount ?? 0) }
        var parts: [String] = []
        if let season, season > 1 { parts.append(L10n.format("Season %d", season)) }
        if beyond == true, let total {
            parts += [L10n.format("%d episodes", held.count), L10n.format("The series has %d", total)]
        } else if let aired, aired > 0 {
            let absent = missing ?? []
            if complete == true { parts.append(aired == total ? L10n.format("Complete · %d episodes", aired) : L10n.format("All %d aired episodes", aired)) }
            else {
                parts.append(L10n.format("%d of %d episodes", aired - absent.count, aired))
                // Whichever is shorter to read: what is absent, or what is there.
                parts.append(absent.count <= held.count ? L10n.text("Missing") + " " + Self.ranges(absent) : Self.ranges(held))
            }
        } else {
            parts += [held.count == 1 ? L10n.text("1 episode") : L10n.format("%d episodes", held.count), Self.ranges(held)]
        }
        if let extras, extras > 0 { parts.append(L10n.format("Extras: %d", extras)) }
        return parts.joined(separator: " · ")
    }
    /// Picture, sound and size per episode: "2160p · Dolby Vision · Surround 5.1 · Per episode 1.8 GB · 5.1 Mb/s · 48 min".
    var qualityLine: String {
        var parts: [String] = []
        if let pictures, pictures.count > 1 { parts.append(pictures.map { "\($0.resolution) ×\($0.count)" }.joined(separator: ", ")) }
        else if let resolution { parts.append(resolution) }
        if let hdr = measured?.video?.hdr ?? hdr { parts.append(hdr) }
        if let fps, fps >= 49 { parts.append("\(fps) fps") }
        // Sound as read from the file when it has been; otherwise as the share's names claim.
        if let track = measured?.bestTrack { parts.append(track.label) }
        else {
            switch audio {
            case "atmos": parts.append(L10n.text("Dolby Atmos"))
            case "stereo": parts.append(L10n.text("Stereo"))
            case let layout?: parts.append(L10n.text("Surround") + " " + layout)
            default: break
            }
        }
        if let codec = measured?.video?.codec ?? codec { parts.append(codec) }
        if let totalBytes, let count = videoCount, count > 1 {
            parts.append(L10n.text("Per episode") + " " + ByteCountFormatter.string(fromByteCount: totalBytes / Int64(count), countStyle: .file))
        }
        if let mbps, mbps > 0 { parts.append(String(format: mbps >= 9.95 ? "%.0f\u{00A0}Mb/s" : "%.1f\u{00A0}Mb/s", mbps)) }
        if let minutes, minutes > 0 { parts.append(L10n.format("%d min", minutes)) }
        return parts.joined(separator: " · ")
    }
}
struct TorrentSearchItem: Decodable, Identifiable {
    var infoHash: String
    let name: String
    let magnet: String
    let sizeBytes: UInt64?
    let sizeLabel: String?
    let seeders: UInt64?
    let leechers: UInt64?
    let sources: [TorrentSearchSource]
    let hints: TorrentReleaseHints
    let matchScore: Int
    let matchNotes: [String]
    /// A cloud-drive share (China): how many videos it holds, and when it was posted.
    var videoCount: Int? = nil
    var added: String? = nil
    /// What this copy of the share holds; absent from servers that list a share as one result.
    var cloud: CloudCopy? = nil
    var isCloud: Bool { sources.first?.provider == "quark" }
    var shareURL: String? { isCloud ? sources.first?.url : nil }
    /// "12 videos · 23.4 GB · 2025-03-02" for a cloud share.
    var cloudSummary: String {
        [videoCount.map { $0 == 1 ? L10n.text("1 video") : L10n.format("%d videos", $0) } ?? "", size,
         added.map { String($0.prefix(10)) } ?? ""].filter { !$0.isEmpty }.joined(separator: " · ")
    }
    var sourceKey: String { sources.map { "\($0.provider):\($0.id)" }.joined(separator: ",") }
    var id: String { infoHash.isEmpty ? sourceKey : infoHash }
    /// Parsed once per release name: several regular expressions, read many times per redraw.
    private static let titles = NSCache<NSString, NSString>()
    var title: String {
        if let show = cloud?.show, !show.isEmpty { return show }
        if let known = Self.titles.object(forKey: name as NSString) { return known as String }
        let parsed = parsedTitle
        Self.titles.setObject(parsed as NSString, forKey: name as NSString)
        return parsed
    }
    private var parsedTitle: String {
        if name.hasPrefix("[BT下载]["), let end = name.dropFirst(7).firstIndex(of: "]") {
            return String(name[name.index(name.startIndex, offsetBy: 7)..<end])
        }
        let chinese = name.replacingOccurrences(of: #"^\[BT下载\]"#, with: "", options: .regularExpression)
        if chinese.range(of: #"[\p{Han}]"#, options: .regularExpression) != nil,
           let bracket = chinese.firstIndex(of: "["), bracket != chinese.startIndex {
            return String(chinese[..<bracket]).trimmingCharacters(in: .whitespacesAndNewlines)
        }
        let cleaned = name.replacingOccurrences(of: #"^(?:www\.)?[^ ]+\.(?:org|com|net)\s*[-–]*\s+"#, with: "", options: .regularExpression)
        let marker = cleaned.range(of: #"(?i)\b(?:(?:19|20)\d{2}|S\d{1,2}(?:E\d+)?|Seasons?\s+\d|2160p|1080[pi]|720p|480p|WEB[ ._-]?DL|BluRay|BRRip|x26[45]|HEVC)\b"#, options: .regularExpression, range: cleaned.isEmpty ? nil : cleaned.index(after: cleaned.startIndex)..<cleaned.endIndex)
        let prefix = marker.map { String(cleaned[..<$0.lowerBound]) } ?? cleaned
        let readable = prefix.replacingOccurrences(of: "[._]+", with: " ", options: .regularExpression)
            .trimmingCharacters(in: CharacterSet.whitespacesAndNewlines.union(CharacterSet(charactersIn: "([{-–")))
        return readable.isEmpty ? name : readable
    }
    /// A whole season (or several), as opposed to individual episodes.
    var isCompleteSeason: Bool {
        // A cloud copy is a whole season only when it holds every aired episode.
        if let cloud, cloud.aired != nil { return cloud.complete == true }
        return hints.episodeCount != nil || (hints.season != nil && hints.episode == nil)
    }
    var isSingleEpisode: Bool { hints.episode != nil && hints.episodeEnd == nil && hints.episodeCount == nil }
    func covers(season wanted: Int) -> Bool {
        guard let season = hints.season else { return false }
        return (season...max(season, hints.seasonEnd ?? season)).contains(wanted)
    }
    func covers(episode wanted: Int) -> Bool {
        // A cloud copy's episodes are known one by one, gaps included.
        if let held = cloud?.episodes { return held.contains(wanted) }
        guard let episode = hints.episode else { return false }
        return (episode...max(episode, hints.episodeEnd ?? episode)).contains(wanted)
    }
    /// What the release contains, season first: "Season 2 Ep 1", "Complete Season 1".
    var scope: String {
        if let count = hints.episodeCount {
            return hints.season.map { L10n.format("Complete Season %d", $0) } ?? L10n.format("Complete · %d episodes", count)
        }
        if let episode = hints.episode {
            switch (hints.season, hints.episodeEnd) {
            case let (season?, end?): return L10n.format("Season %d Ep %d–%d", season, episode, end)
            case let (season?, nil): return L10n.format("Season %d Ep %d", season, episode)
            case let (nil, end?): return L10n.format("Ep %d–%d", episode, end)
            case (nil, nil): return L10n.format("Ep %d", episode)
            }
        }
        if let season = hints.season {
            if let end = hints.seasonEnd, end > season { return L10n.format("Seasons %d–%d", season, end) }
            return L10n.format("Complete Season %d", season)
        }
        return ""
    }
    /// The group that made the release ("HHWEB"), from the end of its name; tells same-sized releases apart.
    var releaseGroup: String {
        var base = name
        for suffix in [".mkv", ".mp4", ".avi"] where base.lowercased().hasSuffix(suffix) { base.removeLast(suffix.count) }
        if let bracket = base.lastIndex(of: "["), base.hasSuffix("]") { base = String(base[..<bracket]) }
        guard let dash = base.lastIndex(of: "-") else { return "" }
        let group = base[base.index(after: dash)...].trimmingCharacters(in: .whitespaces)
        return (2...16).contains(group.count) && group.allSatisfy({ $0.isLetter || $0.isNumber }) ? group : ""
    }
    var edition: String { [scope, hints.year.map(String.init) ?? ""].filter { !$0.isEmpty }.joined(separator: " · ") }
    var quality: String { [hints.resolution, hints.releaseSource, hints.videoCodec].compactMap { $0 }.joined(separator: " · ") }
    var languages: String {
        hints.languages.map { tag in
            switch tag.uppercased() {
            case "ENG": return "English"
            case "CHS": return "Chinese (Simplified)"
            case "CHT": return "Chinese (Traditional)"
            case "JPN": return "Japanese"
            case "MULTI": return "Multiple languages"
            case "DUAL AUDIO", "DUAL-AUDIO", "DUAL_AUDIO": return "Dual audio"
            default: return tag
            }
        }.joined(separator: " · ")
    }
    var availability: String {
        guard let seeders else { return "" }
        return seeders == 0 ? L10n.text("No reported seeds") : seeders == 1 ? L10n.text("1 seed") : L10n.format("%@ seeds", seeders.formatted())
    }
    var size: String { sizeBytes.map { ByteCountFormatter.string(fromByteCount: Int64(clamping: $0), countStyle: .file) } ?? sizeLabel ?? "" }
}
struct TorrentSearchResponse: Decodable {
    struct Provider: Decodable { let provider: String; let count: Int; let error: String? }
    var items: [TorrentSearchItem]
    let providers: [Provider]
    /// The query as searched; a cloud share's files are looked up again under it.
    var query: String? = nil
}
private struct TorrentSearchDetails: Decodable {
    struct File: Decodable { let name: String; let sizeBytes: UInt64?; let hints: TorrentReleaseHints }
    struct Release: Decodable { let item: TorrentSearchItem; let files: [File] }
    let releases: [Release]?
    let description: String
    let files: [File]?
    let audioLanguage: String?
    let subtitleLanguage: String?
    enum CodingKeys: String, CodingKey { case description, files, audioLanguage, subtitleLanguage, releases }
    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        releases = try values.decodeIfPresent([Release].self, forKey: .releases)
        description = try values.decode(String.self, forKey: .description)
        files = try values.decodeIfPresent([File].self, forKey: .files)
        audioLanguage = try? values.decode(String.self, forKey: .audioLanguage)
        subtitleLanguage = try? values.decode(String.self, forKey: .subtitleLanguage)
    }
    var languages: [String] {
        [("Audio", audioLanguage), ("Subtitles", subtitleLanguage)].compactMap { label, value in
            guard let value = value?.trimmingCharacters(in: .whitespacesAndNewlines),
                  !value.isEmpty, !["unknown", "und", "0"].contains(value.lowercased()) else { return nil }
            return "\(label) · \(value)"
        }
    }
}

struct TorrentSearchView: View {
    #if os(iOS)
    @Environment(\.horizontalSizeClass) private var sizeClass
    #endif
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    /// The text being typed. Held outside this view's observed state: typing must not
    /// re-evaluate the whole results page (filters, ownership checks, every row) per keystroke.
    @State private var draft: SearchDraft
    private var query: String { draft.text }
    @State private var year: Int?
    @State private var season: Int?
    @State private var episode: Int?
    init(initialQuery: String = "", year: Int? = nil, season: Int? = nil, episode: Int? = nil) {
        _draft = State(initialValue: SearchDraft(initialQuery)); _year = State(initialValue: year)
        _season = State(initialValue: season); _episode = State(initialValue: episode)
    }
    @AppStorage("torrentSearchMarket") private var market = "world"
    @AppStorage("torrentSearchResolution") private var resolution = "all"
    @State private var source = "all"
    @State private var scopeFilter = "all"
    @State private var seasonFilter: Int?
    @State private var episodeFilter: Int?
    @AppStorage("torrentSearchSort") private var sortOrder = "match"
    @State private var downloaderSeason: Int?
    @State private var searchedQuery = ""
    @State private var showDownloader = false
    @State private var response: TorrentSearchResponse?
    @State private var error: String?
    @State private var searching = false
    @State private var work: Task<Void, Never>?
    @State private var selectedID: String?
    #if os(macOS)
    @State private var trialMode = false
    @State private var trialCandidates: [String: [String: Any]] = [:]
    @State private var showTrials = false
    @State private var startingTrial = false
    #endif
    @FocusState private var searchFocused: Bool
    @State private var prepareNextSearch = false
    #if !os(tvOS)
    #endif
    private var resolutionItems: [TorrentSearchItem] {
        (response?.items ?? []).filter { resolution == "all" || $0.hints.resolution?.lowercased() == resolution }
    }
    private var availableSeasons: [Int] {
        Array(Set(resolutionItems.flatMap { item in item.hints.season.map { Array($0...max($0, item.hints.seasonEnd ?? $0)) } ?? [] })).sorted()
    }
    private var availableEpisodes: [Int] {
        Array(Set(resolutionItems.filter { seasonFilter == nil || $0.covers(season: seasonFilter!) }
            .flatMap { item in item.hints.episode.map { Array($0...max($0, item.hints.episodeEnd ?? $0)) } ?? [] })).sorted()
    }
    private var visibleItems: [TorrentSearchItem] {
        var items = resolutionItems
        if scopeFilter == "seasons" { items = items.filter(\.isCompleteSeason) }
        if scopeFilter == "episodes" { items = items.filter(\.isSingleEpisode) }
        if let seasonFilter { items = items.filter { $0.covers(season: seasonFilter) } }
        if let episodeFilter { items = items.filter { $0.covers(episode: episodeFilter) } }
        switch sortOrder {
        case "episode":
            // Stable: releases of the same episode keep the server's relevance order.
            return items.enumerated().sorted { a, b in
                // Season packs lead their season; releases with no season or episode go last.
                func key(_ item: TorrentSearchItem) -> (Int, Int, Int) {
                    (item.hints.season == nil && item.hints.episode == nil && item.hints.episodeCount == nil ? 1 : 0, item.hints.season ?? 0, item.hints.episode ?? 0)
                }
                let left = key(a.element), right = key(b.element)
                if left != right { return left < right }
                let seeds = (a.element.seeders ?? 0, b.element.seeders ?? 0)
                return seeds.0 != seeds.1 ? seeds.0 > seeds.1 : a.offset < b.offset
            }.map(\.element)
        case "seeds":
            return items.enumerated().sorted { a, b in
                let seeds = (a.element.seeders ?? 0, b.element.seeders ?? 0)
                return seeds.0 != seeds.1 ? seeds.0 > seeds.1 : a.offset < b.offset
            }.map(\.element)
        default: return items
        }
    }
    private static let titleKeys = NSCache<NSString, NSString>()
    static func titleKey(_ value: String) -> String {
        if let known = titleKeys.object(forKey: value as NSString) { return known as String }
        let key = value.lowercased().unicodeScalars.filter { CharacterSet.alphanumerics.contains($0) }.map(String.init).joined()
        titleKeys.setObject(key as NSString, forKey: value as NSString)
        return key
    }
    /// A release without a year borrows the one its same-titled neighbours agree on, so
    /// artwork lookup is not defeated by an ambiguous bare title.
    private func artworkYear(_ item: TorrentSearchItem) -> Int? {
        if let year = item.hints.year { return year }
        let key = Self.titleKey(item.title)
        let years = Set((response?.items ?? []).filter { Self.titleKey($0.title) == key }.compactMap(\.hints.year))
        return years.count == 1 ? years.first : nil
    }
    /// What the user already has of this release: a library copy or an existing torrent.
    private func ownership(_ item: TorrentSearchItem) -> String? {
        if !item.infoHash.isEmpty, store.torrents.contains(where: { $0.hashString.caseInsensitiveCompare(item.infoHash) == .orderedSame }) {
            return "Already in your torrents"
        }
        let key = Self.titleKey(item.title)
        let episodic = item.hints.season != nil || item.hints.episode != nil || item.hints.episodeCount != nil
        guard !key.isEmpty, let title = store.library?.titles.first(where: {
            Self.titleKey($0.title) == key && ($0.kind == "tv") == episodic
                && (episodic || item.hints.year == nil || $0.year == nil || $0.year == item.hints.year.map(String.init))
        }) else { return nil }
        let files = title.assets.filter { $0.role != "extra" && $0.complete }
        guard episodic else { return files.isEmpty ? nil : "In your library" }
        let season = item.hints.season ?? 1
        if item.isCompleteSeason && item.hints.seasonEnd == nil {
            return files.contains { ($0.season ?? 1) == season } ? "In your library" : nil
        }
        if let episode = item.hints.episode {
            let wanted = episode...max(episode, item.hints.episodeEnd ?? episode)
            return wanted.allSatisfy { number in files.contains { ($0.season ?? 1) == season && $0.episode == number } } ? "In your library" : nil
        }
        return nil
    }
    // MARK: Automatic Episode Downloader
    /// Single-episode releases of the best-matching show (a search can return several shows).
    private var downloaderCandidates: [TorrentSearchItem] {
        let episodes = resolutionItems.filter { $0.isSingleEpisode && !$0.magnet.isEmpty && ($0.seeders ?? 0) > 0 }
        guard let lead = episodes.first.map({ Self.titleKey($0.title) }) else { return [] }
        return episodes.filter { Self.titleKey($0.title) == lead }
    }
    private var downloaderSeasons: [Int] { Array(Set(downloaderCandidates.map { $0.hints.season ?? 1 })).sorted() }
    /// The season it works on: the one chosen in the panel, else the page's season filter,
    /// else the latest season, which is usually the one still airing.
    private var activeDownloaderSeason: Int? {
        let seasons = downloaderSeasons
        if let downloaderSeason, seasons.contains(downloaderSeason) { return downloaderSeason }
        if let seasonFilter, seasons.contains(seasonFilter) { return seasonFilter }
        return seasons.last
    }
    /// One entry per episode found: its best-seeded release, and whether the user already has it.
    private var downloaderPicks: [EpisodePick] {
        guard let season = activeDownloaderSeason else { return [] }
        return EpisodePick.choose(downloaderCandidates, season: season, ownership: ownership)
    }
    @ViewBuilder private var episodeDownloader: some View {
        let picks = downloaderPicks
        if let season = activeDownloaderSeason, let show = picks.first?.item.title {
            let pending = picks.filter { $0.owned == nil }.count
            VStack(alignment: .leading, spacing: 8) {
                Label("Automatic Episode Downloader", systemImage: "sparkles").font(.headline)
                Text(verbatim: [show, L10n.format("%d episodes found", picks.count), L10n.format("%d to add", pending),
                                resolution == "all" ? L10n.text("Any resolution") : resolution].joined(separator: " · "))
                    .font(.caption).foregroundStyle(Theme.muted)
                HStack(spacing: 10) {
                    if downloaderSeasons.count > 1 {
                        Menu {
                            Picker("Season", selection: Binding(get: { season }, set: { downloaderSeason = $0 })) {
                                ForEach(downloaderSeasons, id: \.self) { Text("Season \($0)").tag($0) }
                            }
                        } label: { Label("Season \(season)", systemImage: "chevron.up.chevron.down") }
                            .buttonStyle(MonoButtonStyle(compact: true)).fixedSize()
                    } else {
                        Text("Season \(season)").font(.subheadline.weight(.semibold))
                    }
                    Button(L10n.format("Review %d episodes", picks.count)) { showDownloader = true }
                        .buttonStyle(MonoButtonStyle(prominent: true, compact: true))
                    Spacer(minLength: 0)
                }
            }.padding(12).frame(maxWidth: .infinity, alignment: .leading)
                .background(Theme.panel, in: RoundedRectangle(cornerRadius: 10))
                .sheet(isPresented: $showDownloader) {
                    LocalizedInterface { EpisodeDownloaderSheet(show: show, season: season, picks: picks) }.environmentObject(store)
                }
        }
    }
    private var selectedItem: TorrentSearchItem? { visibleItems.first { $0.id == selectedID } }
    var body: some View {
        GeometryReader { geometry in
            VStack(spacing: 0) {
                searchHeader
                #if os(macOS)
                HStack(spacing: 12) {
                    Toggle("Compare releases", isOn: $trialMode).toggleStyle(.button)
                    if trialMode {
                        Text("\(trialCandidates.count) selected").font(.caption)
                        Button("Start trial") { Task { await startTrial() } }
                            .disabled(!(3...5).contains(trialCandidates.count) || startingTrial)
                        Button("Clear selection") { trialCandidates.removeAll() }.disabled(trialCandidates.isEmpty)
                    }
                    Spacer()
                    Button("Trials") { showTrials = true }
                }.padding(.horizontal, 16).padding(.bottom, 8)
                #endif
                if let response {
                    if visibleItems.isEmpty && !searching {
                        if !response.providers.isEmpty && response.providers.allSatisfy({ $0.error != nil }) {
                            let limited = response.providers.contains { $0.error?.contains("rate limiting") == true }
                            ContentUnavailableView(limited ? "Search rate limited" : "Sources unavailable", systemImage: limited ? "clock" : "network.slash", description: Text(LocalizedStringKey(response.providers.compactMap(\.error).first ?? "Try again shortly.")))
                        } else if !response.items.isEmpty {
                            // Everything found is filtered out: say so, or it reads as a failed search.
                            ContentUnavailableView {
                                Label("\(response.items.count) releases hidden by filters", systemImage: "line.3.horizontal.decrease.circle")
                            } description: {
                                Text("None of them match the resolution or other filters you have chosen.")
                            } actions: {
                                Button("Show all") { resolution = "all"; scopeFilter = "all"; seasonFilter = nil; episodeFilter = nil; selectedID = visibleItems.first?.id }
                            }
                        } else {
                            ContentUnavailableView("No releases found", systemImage: "magnifyingglass", description: Text("Try a shorter title or another source."))
                        }
                    } else if usesColumns(geometry.size.width) {
                        HStack(spacing: 0) {
                            results(visibleItems, inline: true)
                                .frame(width: min(440, max(280, geometry.size.width * 0.43)))
                            Divider()
                            if let selectedItem, selectedItem.isCloud {
                                CloudShareView(item: selectedItem, query: response.query ?? query, embedded: true).id(selectedItem.id)
                            } else if let selectedItem {
                                TorrentSearchDetail(item: selectedItem, embedded: true, onAdded: { prepareNextSearch = true; focusNextSearch() }).id(selectedItem.id)
                            } else {
                                ContentUnavailableView("Choose a release", systemImage: "text.magnifyingglass")
                                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                            }
                        }
                    } else {
                        results(visibleItems, inline: false)
                    }
                } else {
                    ContentUnavailableView("Find your next watch", systemImage: "magnifyingglass", description: Text("Search movies and TV shows."))
                }
            }
        }
        .navigationTitle("Find")
        .onAppear { if prepareNextSearch { focusNextSearch() } }
        .onDisappear { work?.cancel(); searching = false }
        .task { if !query.isEmpty && response == nil { search() } }
        #if os(macOS)
        .sheet(isPresented: $showTrials) { LocalizedInterface { TorrentTrialsView() } }
        .task {while !Task.isCancelled {await deduplicateKnownHashes();try? await Task.sleep(for:.seconds(5))}}
        #endif
    }
    private func usesColumns(_ width: CGFloat) -> Bool {
        #if os(macOS)
        if trialMode { return false }
        #endif
        #if os(tvOS)
        return false
        #elseif os(iOS)
        return UIDevice.current.userInterfaceIdiom == .pad && width >= 640
        #else
        return width >= 640
        #endif
    }
    /// World or China, beside the search box. A narrow window keeps just the globe and flag.
    private var regionSelector: some View {
        HStack(spacing: 2) {
            regionOption("world", name: "World") { Image(systemName: "globe") }
            regionOption("china", name: "China") { Text(verbatim: "🇨🇳") }
        }.padding(3).background(Theme.control, in: RoundedRectangle(cornerRadius: Theme.radius))
            .fixedSize()
            .accessibilityElement(children: .contain).accessibilityLabel("Search region")
    }
    private func regionOption<Icon: View>(_ value: String, name: String, @ViewBuilder icon: () -> Icon) -> some View {
        let selected = market == value
        return Button { market = value } label: {
            HStack(spacing: 6) {
                icon()
                if !compactRegion { Text(LocalizedStringKey(name)) }
            }.font(.subheadline.weight(.semibold)).padding(.horizontal, 10).frame(minHeight: regionHeight)
                .foregroundStyle(selected ? Theme.onAccent : Color.white)
                .background(selected ? Theme.accent : .clear, in: RoundedRectangle(cornerRadius: 6))
                .contentShape(Rectangle())
        }.buttonStyle(.plain)
            .accessibilityLabel(LocalizedStringKey(name)).accessibilityAddTraits(selected ? .isSelected : [])
    }
    private var regionHeight: CGFloat {
        #if os(tvOS)
        56
        #else
        38
        #endif
    }
    private var compactRegion: Bool {
        #if os(iOS)
        sizeClass == .compact
        #else
        false
        #endif
    }
    private var searchHeader: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 10) {
              regionSelector
                .onChange(of: market) {
                    work?.cancel(); searching = false; response = nil; selectedID = nil; error = nil; source = "all"
                    if query.trimmingCharacters(in: .whitespacesAndNewlines).count >= 2 { search() }
                }
              SearchBox(draft: draft, searching: searching, focused: $searchFocused, search: search,
                        edited: {
                            if prepareNextSearch { year = nil; season = nil; episode = nil; prepareNextSearch = false }
                        },
                        clear: {
                            // Clears the box and results, ready for the next search.
                            work?.cancel(); searching = false
                            draft.text = ""; response = nil; selectedID = nil; error = nil
                            resetFilters()
                            focusNextSearch()
                        })
            }
            ViewThatFits(in: .horizontal) {
                HStack { if market != "china" { sourcePicker }; resolutionPicker; Spacer(); searchStatus }
                VStack(alignment: .leading, spacing: 6) {
                    HStack { if market != "china" { sourcePicker.labelsHidden() }; Spacer(); resolutionPicker.labelsHidden() }
                    searchStatus
                }
            }
            MediaDriveSpace(compact: true)
            if response != nil { refinements }
            if year != nil || season != nil || episode != nil {
                HStack {
                    Text([year.map { "\($0)" }, season.map { "Season \($0)" }, episode.map { "Episode \($0)" }].compactMap { $0 }.joined(separator: " · "))
                    Button("Clear filters") { year = nil; season = nil; episode = nil; search() }
                }.font(.caption).foregroundStyle(.secondary)
            }
            if let error { Text(L10n.text(error)).font(.caption).foregroundStyle(.orange) }
            if let response {
                ForEach(response.providers.filter { $0.error != nil }, id: \.provider) { provider in
                    Text("\(TorrentSearchSource.label(provider.provider)): \(provider.error ?? "")")
                        .font(.caption).foregroundStyle(.secondary)
                }
            }
        }.padding(16)
    }
    private var sourcePicker: some View {
                Picker("Sources", selection: $source) {
                    Text("Recommended").tag("all")
                    Text("The Pirate Bay").tag("tpb")
                    Text("Torrent CSV").tag("csv")
                    Text("Nyaa").tag("nyaa")
                }
                #if os(tvOS)
                .pickerStyle(.segmented)
                #else
                .pickerStyle(.menu).fixedSize().searchFilterControl()
                #endif
    }
    private var resolutionPicker: some View {
                Picker("Resolution", selection: $resolution) {
                    Text("All resolutions").tag("all")
                    Text(verbatim:"2160p").tag("2160p")
                    Text(verbatim:"1080p").tag("1080p")
                    Text(verbatim:"720p").tag("720p")
                }.pickerStyle(.menu).fixedSize().searchFilterControl()
                    .onChange(of: resolution) { selectedID = visibleItems.first?.id }
    }
    @ViewBuilder private var refinements: some View {
        ViewThatFits(in: .horizontal) {
            HStack { scopePicker; seasonPicker; episodePicker; sortPicker; Spacer() }
            VStack(alignment: .leading, spacing: 6) {
                HStack { scopePicker.labelsHidden(); seasonPicker.labelsHidden(); Spacer() }
                HStack { episodePicker.labelsHidden(); sortPicker.labelsHidden(); Spacer() }
            }
        }
        episodeDownloader
    }
    private var scopePicker: some View {
        Picker("Show", selection: $scopeFilter) {
            Text("All releases").tag("all")
            Text("Complete seasons").tag("seasons")
            Text("Single episodes").tag("episodes")
        }.pickerStyle(.menu).fixedSize().searchFilterControl()
            .onChange(of: scopeFilter) { selectedID = visibleItems.first?.id }
    }
    @ViewBuilder private var seasonPicker: some View {
        if availableSeasons.count > 1 {
            Picker("Season", selection: $seasonFilter) {
                Text("All seasons").tag(Int?.none)
                ForEach(availableSeasons, id: \.self) { Text("Season \($0)").tag(Int?.some($0)) }
            }.pickerStyle(.menu).fixedSize().searchFilterControl()
                .onChange(of: seasonFilter) { episodeFilter = nil; selectedID = visibleItems.first?.id }
        }
    }
    @ViewBuilder private var episodePicker: some View {
        if availableEpisodes.count > 1 {
            Picker("Episode", selection: $episodeFilter) {
                Text("All episodes").tag(Int?.none)
                ForEach(availableEpisodes, id: \.self) { Text("Episode \($0)").tag(Int?.some($0)) }
            }.pickerStyle(.menu).fixedSize().searchFilterControl()
                .onChange(of: episodeFilter) { selectedID = visibleItems.first?.id }
        }
    }
    private var sortPicker: some View {
        Picker("Sort", selection: $sortOrder) {
            Text("Best match").tag("match")
            Text("Episode order").tag("episode")
            Text("Most seeds").tag("seeds")
        }.pickerStyle(.menu).fixedSize().searchFilterControl()
    }
    @ViewBuilder private var searchStatus: some View {
                if let response, !searching {
                    // "3 of 12 releases" when filters hide some of what was found.
                    let shown = visibleItems.count, found = response.items.count
                    (shown < found ? Text("\(shown) of \(found) releases") : Text("\(shown) releases")).font(.caption).foregroundStyle(.secondary)
                }
                if searching {
                    ProgressView().controlSize(.small)
                    // Every share is opened to check its files, which takes a while.
                    if market == "china" { Text("Searching and checking links…").font(.caption).foregroundStyle(.secondary) }
                }
    }
    private func results(_ items: [TorrentSearchItem], inline: Bool) -> some View {
        List(items) { item in
            #if os(macOS)
            if trialMode {
                TorrentSearchRow(item: item, onTrial: { kind, value, name, key in
                    if trialCandidates[value] != nil {trialCandidates.removeValue(forKey: value)}
                    else if trialCandidates.count < 5 {trialCandidates[value] = ["name":name,"source":[kind:value],"listing_key":key]}
                }, trialSelected: Set(trialCandidates.keys))
            } else { ordinaryRow(item, inline: inline) }
            #else
            ordinaryRow(item, inline: inline)
            #endif
        }.listStyle(.plain).canvasBackground().disabled(searching)
    }
    @ViewBuilder private func ordinaryRow(_ item: TorrentSearchItem, inline: Bool) -> some View {
            if item.isCloud && !inline {
                NavigationLink { CloudShareView(item: item, query: response?.query ?? query) } label: { TorrentSearchRow(item: item, ownership: ownership(item), artworkYear: artworkYear(item)) }
            } else if inline {
                TorrentSearchRow(item: item, onAdded: { prepareNextSearch = true; focusNextSearch() }, ownership: ownership(item), artworkYear: artworkYear(item))
                    .contentShape(Rectangle()).onTapGesture { selectedID = item.id }
                    .listRowBackground(selectedID == item.id ? Theme.accent.opacity(0.13) : Color.clear)
                    .accessibilityAddTraits(selectedID == item.id ? .isSelected : [])
            } else {
                NavigationLink { TorrentSearchDetail(item: item, onAdded: { prepareNextSearch = true }).id(item.id) } label: { TorrentSearchRow(item: item, onAdded: { prepareNextSearch = true; focusNextSearch() }, ownership: ownership(item), artworkYear: artworkYear(item)) }
            }
    }
    #if os(macOS)
    private func deduplicateKnownHashes() async {
        guard response != nil else {return}
        do {
            struct Known:Decodable {struct Entry:Decodable {let listingKey:String;let hash:String};let hashes:[Entry]}
            let known:Known=try await store.request("GET","/v1/search/torrents/hashes")
            guard var response else {return}
            var seen=Set<String>()
            response.items=response.items.compactMap { item in
                var item=item
                let key=item.sources.map {"\($0.provider):\($0.id)"}.joined(separator:",")
                if item.infoHash.isEmpty, let match=known.hashes.first(where:{$0.listingKey==key}) {item.infoHash=match.hash}
                if !item.infoHash.isEmpty && !seen.insert(item.infoHash.lowercased()).inserted {return nil}
                return item
            }
            self.response=response
        } catch { /* Hash enrichment must never interrupt search. */ }
    }
    private func startTrial() async {
        startingTrial = true
        defer {startingTrial = false}
        do {
            let _: TorrentTrial = try await store.request("POST", "/v1/trials", body: ["candidates":Array(trialCandidates.values)], timeout: 30)
            trialCandidates.removeAll(); showTrials = true
        } catch {self.error = error.localizedDescription}
    }
    #endif
    private func focusNextSearch() {
        #if !os(tvOS)
        Task { @MainActor in
            // Allow the phone's detail navigation to finish before opening its keyboard.
            await Task.yield()
            searchFocused = true
            draft.selection = TextSelection(range: draft.text.startIndex..<draft.text.endIndex)
        }
        #endif
    }
    /// Show, season, episode and sort describe one search's results, so a different search
    /// starts without them. Sources and resolution are standing preferences and are kept.
    private func resetFilters() {
        scopeFilter = "all"; seasonFilter = nil; episodeFilter = nil; downloaderSeason = nil; sortOrder = "match"
    }
    private func search() {
        searchFocused = false
        guard query.trimmingCharacters(in: .whitespacesAndNewlines).count >= 2 else { return }
        let wanted = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        if wanted != searchedQuery { resetFilters(); searchedQuery = wanted }
        work?.cancel(); searching = true; error = nil
        var body: [String: Any] = ["query": query, "market": market, "sources": source == "all" ? [] : [source]]
        if let year { body["year"] = year }; if let season { body["season"] = season }; if let episode { body["episode"] = episode }
        work = Task { @MainActor in
            do {
                let value: TorrentSearchResponse = try await store.request("POST", "/v1/search/torrents", body: body, timeout: market == "china" ? 90 : 30)
                guard !Task.isCancelled else { return }; response = value
                if let seasonFilter, !availableSeasons.contains(seasonFilter) { self.seasonFilter = nil }
                if let episodeFilter, !availableEpisodes.contains(episodeFilter) { self.episodeFilter = nil }
                if !visibleItems.contains(where: { $0.id == selectedID }) { selectedID = visibleItems.first?.id }
            } catch {
                guard !Task.isCancelled else { return }; self.error = error.localizedDescription
            }
            searching = false
        }
    }
}

/// One episode in the Automatic Episode Downloader: its best-seeded release.
struct EpisodePick: Identifiable {
    let episode: Int
    let item: TorrentSearchItem
    let owned: String?
    /// Every release found for this episode, best seeded first; `item` is the suggested one.
    var alternatives: [TorrentSearchItem] = []
    var id: Int { episode }
    /// One pick per episode of `season` among single-episode releases: the best seeded, unless it is
    /// far larger than the season's usual episode and a well-seeded release of ordinary size exists.
    static func choose(_ candidates: [TorrentSearchItem], season: Int, only wanted: Set<Int>? = nil,
                       ownership: (TorrentSearchItem) -> String? = { _ in nil }) -> [EpisodePick] {
        var options: [Int: [TorrentSearchItem]] = [:], owned: [Int: String] = [:]
        for item in candidates where (item.hints.season ?? 1) == season {
            guard let episode = item.hints.episode, wanted?.contains(episode) ?? true else { continue }
            if let status = ownership(item), owned[episode] != "In your library" { owned[episode] = status }
            options[episode, default: []].append(item)
        }
        for episode in options.keys {
            options[episode]?.sort { ($0.seeders ?? 0) > ($1.seeders ?? 0) }
            // The same release re-uploaded under another hash is not a different choice: keep its best-seeded copy.
            var seen = Set<String>()
            options[episode]?.removeAll { item in
                !seen.insert("\((item.sizeBytes ?? 0) >> 24)|\(item.quality)|\(item.hints.audioLabel)|\(item.releaseGroup)").inserted
            }
        }
        // One oversized remux should not stand out from the rest of the season.
        let leaders = options.values.compactMap { $0.first?.sizeBytes }.sorted()
        let usual = leaders.isEmpty ? 0 : Double(leaders[leaders.count / 2])
        return options.keys.sorted().compactMap { number in
            guard let all = options[number], var choice = all.first else { return nil }
            if usual > 0, let size = choice.sizeBytes, Double(size) > usual * 2,
               let ordinary = all.first(where: { item in
                   guard let bytes = item.sizeBytes else { return false }
                   return (usual * 0.5...usual * 1.6).contains(Double(bytes)) && (item.seeders ?? 0) * 4 >= (choice.seeders ?? 0)
               }) { choice = ordinary }
            return EpisodePick(episode: number, item: choice, owned: owned[number], alternatives: all)
        }
    }
}

/// Lists what the downloader found, to add all at once or one at a time.
struct EpisodeDownloaderSheet: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) private var dismiss
    let show: String
    let season: Int
    let picks: [EpisodePick]
    /// Missing episodes no release was found for (the missing-episodes search only).
    var notFound: [Int] = []
    /// What the missing-episodes search is still doing; nil once it has finished.
    var progress: String? = nil
    /// Missing episodes found in a Quark Drive share instead (the missing-episodes search only).
    var cloudOffer: CloudOffer? = nil
    @State private var adding: Set<Int> = []
    @State private var added: Set<Int> = []
    @State private var failures: [Int: String] = [:]
    /// Releases the user chose in place of the suggested ones, by episode.
    @State private var chosen: [Int: TorrentSearchItem] = [:]
    private func release(_ pick: EpisodePick) -> TorrentSearchItem { chosen[pick.episode] ?? pick.item }
    private func summary(_ item: TorrentSearchItem) -> String {
        [item.size, item.availability, item.quality, item.hints.audioLabel, item.releaseGroup].filter { !$0.isEmpty }.joined(separator: " · ")
    }
    private var pending: [EpisodePick] { picks.filter { $0.owned == nil && !added.contains($0.episode) } }
    var body: some View {
        NavigationStack {
            List {
                if let cloudOffer { CloudOfferSection(offer: cloudOffer) }
                SwiftUI.Section {
                    ForEach(picks) { pick in row(pick) }
                    if let progress {
                        HStack(spacing: 10) { ProgressView().controlSize(.small); Text(LocalizedStringKey(progress)).font(.callout).foregroundStyle(Theme.muted) }
                            .padding(.vertical, 4)
                    }
                    ForEach(notFound, id: \.self) { episode in
                        HStack {
                            Text(L10n.format("Season %d Ep %d", season, episode)).font(.headline)
                            Spacer()
                            Label("No release found", systemImage: "questionmark.circle").font(.caption.weight(.semibold)).foregroundStyle(Theme.muted)
                        }.padding(.vertical, 4)
                    }
                } header: {
                    HStack {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(verbatim: show + " · " + L10n.format("Season %d", season)).font(.subheadline.weight(.semibold))
                            // What "Add all" will download, following any releases chosen below.
                            if !pending.isEmpty {
                                let total = pending.reduce(UInt64(0)) { $0 + (release($1).sizeBytes ?? 0) }
                                Text(verbatim: [L10n.format("%d to add", pending.count),
                                                total > 0 ? ByteCountFormatter.string(fromByteCount: Int64(clamping: total), countStyle: .file) : ""]
                                    .filter { !$0.isEmpty }.joined(separator: " · "))
                                    .font(.caption).monospacedDigit().foregroundStyle(Theme.muted)
                            }
                        }
                        Spacer()
                        Button(LocalizedStringKey(pending.isEmpty ? "Nothing to add" : "Add all")) { Task { for pick in pending { await add(pick) } } }
                            .buttonStyle(MonoButtonStyle(prominent: true, compact: true))
                            .disabled(pending.isEmpty || !adding.isEmpty)
                    }.textCase(nil).padding(.vertical, 6)
                }
            }
            .canvasBackground()
            .navigationTitle("Automatic Episode Downloader")
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            #endif
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Done") { dismiss() } } }
        }
        #if os(macOS)
        .frame(minWidth: 620, minHeight: 480)
        #endif
    }
    private func row(_ pick: EpisodePick) -> some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 3) {
                Text(L10n.format("Season %d Ep %d", season, pick.episode)).font(.headline)
                Text(verbatim: release(pick).name).font(.caption).foregroundStyle(Theme.muted).lineLimit(2)
                Text(verbatim: summary(release(pick))).font(.caption).monospacedDigit()
                if pick.owned == nil, !added.contains(pick.episode), pick.alternatives.count > 1 {
                    // Another release for this episode: the suggestion is only the best seeded of ordinary size.
                    Menu {
                        ForEach(pick.alternatives) { option in
                            Button { chosen[pick.episode] = option } label: {
                                if option.id == release(pick).id { Label(summary(option), systemImage: "checkmark") } else { Text(verbatim: summary(option)) }
                            }
                        }
                    } label: {
                        HStack(spacing: 5) {
                            Image(systemName: "arrow.left.arrow.right")
                            Text(verbatim: L10n.format("Choose another (%d)", pick.alternatives.count))
                        }.font(.caption.weight(.semibold)).foregroundStyle(Theme.accent).padding(.vertical, 4)
                    }.menuStyle(.borderlessButton).fixedSize()
                }
                if let failure = failures[pick.episode] { Text(failure).font(.caption).foregroundStyle(.orange) }
            }
            Spacer(minLength: 8)
            if let owned = pick.owned {
                Label(LocalizedStringKey(owned), systemImage: "checkmark.circle.fill").font(.caption.weight(.semibold)).foregroundStyle(Theme.muted)
            } else if added.contains(pick.episode) {
                Label("Added", systemImage: "checkmark.circle.fill").font(.caption.weight(.semibold))
            } else if adding.contains(pick.episode) {
                ProgressView().controlSize(.small)
            } else {
                Button { Task { await add(pick) } } label: { Label("Add", systemImage: "plus").labelStyle(.titleAndIcon) }
                    .buttonStyle(MonoButtonStyle(compact: true)).fixedSize()
            }
        }.padding(.vertical, 4)
    }
    private func add(_ pick: EpisodePick) async {
        guard pick.owned == nil, !added.contains(pick.episode), !adding.contains(pick.episode) else { return }
        adding.insert(pick.episode); failures[pick.episode] = nil
        defer { adding.remove(pick.episode) }
        do {
            struct Result: Decodable {}
            let _: Result = try await store.request("POST", "/v1/torrents", body: ["magnet": release(pick).magnet, "paused": false], timeout: 45)
            added.insert(pick.episode)
        } catch { failures[pick.episode] = error.localizedDescription }
    }
}

/// The search text and its selection, observed only by the search box.
final class SearchDraft: ObservableObject {
    @Published var text: String
    #if !os(tvOS)
    @Published var selection: TextSelection?
    #endif
    init(_ text: String) { self.text = text }
}
/// The search field with its clear and Search buttons. It alone redraws while typing.
private struct SearchBox: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @ObservedObject var draft: SearchDraft
    let searching: Bool
    var focused: FocusState<Bool>.Binding
    let search: () -> Void
    let edited: () -> Void
    let clear: () -> Void
    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: "magnifyingglass").foregroundStyle(Theme.accent)
            field.focused(focused).onSubmit(search).autocorrectionDisabled().textFieldStyle(.plain)
                .onChange(of: draft.text) { _, _ in edited() }
            #if !os(tvOS)
            if !draft.text.isEmpty {
                Button(action: clear) { Image(systemName: "xmark.circle.fill").font(.title3).foregroundStyle(Theme.muted) }
                    .buttonStyle(.plain).accessibilityLabel("Clear search")
            }
            #endif
            Button(action: search) { Text(LocalizedStringKey(searching ? "Searching…" : "Search")) }
                .buttonStyle(MonoButtonStyle(prominent: true))
                .disabled(draft.text.trimmingCharacters(in: .whitespacesAndNewlines).count < 2 || searching)
        }.padding(12).background(Theme.panel, in: RoundedRectangle(cornerRadius: 10))
    }
    @ViewBuilder private var field: some View {
        #if os(tvOS)
        TextField("Movie, show, or release", text: $draft.text)
        #else
        TextField("Movie, show, or release", text: $draft.text, selection: $draft.selection)
        #endif
    }
}
private extension View {
    /// On iPhone and iPad a menu picker is bare text; give it the bordered pop-up look it has on the Mac.
    @ViewBuilder func searchFilterControl() -> some View {
        #if os(iOS)
        self.tint(.white).padding(.horizontal, 4).frame(minHeight: 34)
            .background(Theme.control, in: RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).stroke(.white.opacity(0.12), lineWidth: 1))
        #else
        self
        #endif
    }
}

/// A title's running time from the metadata lookup that also finds its poster.
struct SearchReleaseFacts {
    var runtimeMinutes: Double?
    var episodeMinutes: Double?
    var seasonEpisodes: Int?
}
private struct SearchReleaseArtwork: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    let name: String
    let identity: String
    var season: Int? = nil
    var onFacts: (SearchReleaseFacts) -> Void = { _ in }
    @State private var data: Data?
    private static var facts: [String: SearchReleaseFacts] = [:]
    private static let cache = NSCache<NSString, NSData>()
    private static var inFlight: [String: Task<Data?, Never>] = [:]
    var body: some View {
        Group {
            if let data {
                #if os(macOS)
                if let image = NSImage(data: data) { Image(nsImage: image).resizable().scaledToFill() }
                #else
                if let image = UIImage(data: data) { Image(uiImage: image).resizable().scaledToFill() }
                #endif
            } else {
                RoundedRectangle(cornerRadius: 7).fill(Theme.panel)
                    .overlay(Image(systemName: "film").foregroundStyle(Theme.muted))
            }
        }.frame(width: 34, height: 51).clipShape(RoundedRectangle(cornerRadius: 7))
            .accessibilityHidden(true)
            .task(id: store.server + identity) {
                data = nil
                let key = store.server + identity
                if let known = Self.facts[key] { onFacts(known) }
                if let cached = Self.cache.object(forKey: key as NSString) { data = cached as Data; return }
                let request: Task<Data?, Never>
                if let existing=Self.inFlight[key] {request=existing} else {
                    request=Task { @MainActor in
                        struct Artwork: Decodable { let data: String?; let runtimeMinutes: Double?; let episodeMinutes: Double?; let seasonEpisodes: Int? }
                        defer {Self.inFlight[key]=nil}
                        do {
                            var body: [String: Any] = ["name": name]
                            if let season { body["season"] = season }
                            let artwork: Artwork = try await store.request("POST", "/v1/search/torrents/artwork", body: body, timeout: 20)
                            if Self.facts.count > 400 { Self.facts.removeAll() }
                            Self.facts[key] = SearchReleaseFacts(runtimeMinutes: artwork.runtimeMinutes, episodeMinutes: artwork.episodeMinutes, seasonEpisodes: artwork.seasonEpisodes)
                            guard let encoded=artwork.data,let decoded=Data(base64Encoded:encoded) else {return nil}
                            Self.cache.totalCostLimit=8*1024*1024
                            Self.cache.setObject(decoded as NSData,forKey:key as NSString,cost:decoded.count)
                            return decoded
                        } catch {return nil}
                    }
                    Self.inFlight[key]=request
                }
                let loaded=await request.value
                if !Task.isCancelled {data=loaded; if let known = Self.facts[key] { onFacts(known) }}
            }
    }
}

private struct TorrentSearchRow: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    let item: TorrentSearchItem
    var onAdded: () -> Void = {}
    var onTrial: ((String, String, String, String) -> Void)? = nil
    var trialSelected: Set<String> = []
    var ownership: String? = nil
    var artworkYear: Int? = nil
    @State private var adding = false
    @State private var added = false
    @State private var addError: String?
    @State private var addNote: String?
    #if os(iOS)
    @Environment(\.horizontalSizeClass) private var rowSizeClass
    private var compactRow: Bool { rowSizeClass == .compact }
    #else
    private let compactRow = false
    #endif
    private var displayedSize: String { item.isCloud && item.cloud == nil ? "" : item.size }
    private var metadata: String {
        if let cloud = item.cloud { return cloud.qualityLine }
        return item.isCloud ? item.cloudSummary : [item.edition, item.quality, estimatedBitrate].filter { !$0.isEmpty }.joined(separator: " · ")
    }
    @State private var facts: SearchReleaseFacts?
    /// The picture's likely bitrate: the release's size over its running time, less a typical figure for its kind of audio.
    private var estimatedBitrate: String {
        guard let bytes = item.sizeBytes, bytes > 0, let facts else { return "" }
        let hints = item.hints
        var minutes: Double?
        if hints.season != nil || hints.episodeCount != nil {
            guard let each = facts.episodeMinutes, hints.seasonEnd == nil || hints.seasonEnd == hints.season else { return "" }
            if let first = hints.episode { minutes = each * Double(max(1, (hints.episodeEnd ?? first) - first + 1)) }
            else if let count = hints.episodeCount ?? facts.seasonEpisodes { minutes = each * Double(count) }
        } else { minutes = facts.runtimeMinutes }
        guard let minutes, minutes > 0 else { return "" }
        let total = Double(bytes) * 8 / (minutes * 60)
        let audio: Double = switch hints.audioKind {
        case "lossless_atmos", "dtsx": 4_500_000
        case "lossless": hints.audioChannels == "7.1" ? 4_500_000 : hints.audioChannels == "2.0" ? 1_000_000 : 3_500_000
        case "atmos": 770_000
        case "surround": 640_000
        case "stereo": 160_000
        case "mono": 100_000
        default: 400_000
        }
        let video = max(total * 0.5, total - audio)
        guard (150_000...200_000_000).contains(video) else { return "" }
        return video >= 9_950_000 ? String(format: "≈\u{00A0}%.0f\u{00A0}Mb/s", video / 1_000_000) : video >= 1_000_000 ? String(format: "≈\u{00A0}%.1f\u{00A0}Mb/s", video / 1_000_000) : String(format: "≈\u{00A0}%.0f\u{00A0}kb/s", video / 1000)
    }
    private var tracks: String { [item.hints.audioLabel, item.languages].filter { !$0.isEmpty }.joined(separator: " · ") }
    var body: some View {
        HStack(alignment: .center, spacing: 10) {
            let episodic = item.hints.season != nil || item.hints.episodeCount != nil
            // A year borrowed from same-titled results is sent as a plain "Title Year S01" name.
            // A cloud share's own name says little; the series' title and year find its artwork.
            SearchReleaseArtwork(name: item.cloud?.show != nil && item.hints.year != nil ? "\(item.title) \(item.hints.year!) S\(String(format: "%02d", item.hints.season ?? 1))" : item.hints.year == nil && artworkYear != nil ? "\(item.title) \(artworkYear!)\(episodic ? " S01" : "")" : item.name,
                identity: "\(item.title.lowercased()):\((item.hints.year ?? artworkYear).map(String.init) ?? ""):\(episodic ? "tv:\(item.hints.season ?? 0)" : "movie")",
                season: item.hints.season, onFacts: { facts = $0 })
            VStack(alignment: .leading, spacing: 2) {
                Text(item.title).font(.title3.bold()).foregroundStyle(.primary).lineLimit(2)
                if let cloud = item.cloud {
                    // Which episodes it holds is what decides a choice, so it leads.
                    Text(verbatim: cloud.episodesLine).font(.headline).monospacedDigit()
                        .foregroundStyle(cloud.complete == true ? Theme.accent : cloud.aired == nil ? Color.primary : Color.orange)
                        .fixedSize(horizontal: false, vertical: true)
                }
                if !displayedSize.isEmpty || item.seeders != nil {
                    HStack(alignment: .firstTextBaseline, spacing: 12) {
                        if !displayedSize.isEmpty {
                            Text(verbatim: displayedSize).foregroundStyle(Theme.accent)
                        }
                        if item.seeders != nil {
                            Text(item.availability).foregroundStyle(item.seeders == 0 ? Color.orange : Theme.accent)
                        }
                    }
                    .font(.headline).monospacedDigit()
                    .fixedSize(horizontal: false, vertical: true)
                }
                if !metadata.isEmpty { Text(metadata).font(.caption).foregroundStyle(.secondary).lineLimit(2) }
                if !tracks.isEmpty { Text(tracks).font(.caption).foregroundStyle(.secondary).lineLimit(2) }
                if item.cloud != nil {
                    // The sharer's own description, and when it was shared.
                    Text(verbatim: [item.name, item.added.map { String($0.prefix(10)) } ?? ""].filter { !$0.isEmpty }.joined(separator: " · "))
                        .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
                if let ownership {
                    Label(LocalizedStringKey(ownership), systemImage: "checkmark.circle.fill").font(.caption.weight(.semibold)).foregroundStyle(Theme.accent)
                }
                if let addError { Text(addError).font(.caption).foregroundStyle(.orange) }
                if added && item.isCloud {
                    // A warning when it was already queued or already in the library; otherwise where to watch it.
                    if let addNote { Label { Text(verbatim: addNote) } icon: { Image(systemName: "exclamationmark.circle.fill") }.font(.caption.weight(.semibold)).foregroundStyle(.orange) }
                    else { Label("Follow its progress in Downloading.", systemImage: "checkmark.circle.fill").font(.caption.weight(.semibold)).foregroundStyle(Theme.accent) }
                }
            }
            if let onTrial, !item.isCloud, !item.magnet.isEmpty {
                Spacer(minLength: 8)
                Button(trialSelected.contains(item.magnet) ? "Selected" : "Add to trial") { onTrial("magnet",item.magnet,item.name,item.sourceKey) }
            }
            if item.isCloud {
                Spacer(minLength: 8)
                #if os(tvOS)
                // Choosing episodes happens on the share's page; this row leads there.
                Image(systemName: "icloud.and.arrow.down").foregroundStyle(Theme.accent).accessibilityLabel("Quark Drive")
                #else
                // Downloads every episode of this copy; the share's page is for choosing some.
                if adding { ProgressView().controlSize(.small) }
                else if added { Image(systemName: "checkmark.circle.fill").foregroundStyle(Theme.accent).accessibilityLabel("Added to downloads") }
                else {
                    Button { Task { await addCloud() } } label: {
                        if compactRow { Image(systemName: "icloud.and.arrow.down").frame(width: 22, height: 22) }
                        else { Label("Download", systemImage: "icloud.and.arrow.down").labelStyle(.titleAndIcon) }
                    }.buttonStyle(MonoButtonStyle(prominent: true, compact: true)).fixedSize().accessibilityLabel("Download").pointingHandOnHover()
                }
                #endif
            }
            #if !os(tvOS)
            // Add straight from the list; opening the details stays optional.
            if onTrial == nil, !item.isCloud, !item.magnet.isEmpty {
                Spacer(minLength: 8)
                if adding { ProgressView().controlSize(.small) }
                else if added || ownership == "Already in your torrents" {
                    Image(systemName: "checkmark.circle.fill").foregroundStyle(Theme.accent).accessibilityLabel("Added")
                } else {
                    // Quieter than the page's primary buttons, since every row has one.
                    Button { Task { await addMagnet() } } label: {
                        if compactRow { Image(systemName: "plus").frame(width: 22, height: 22) }
                        else { Label("Add torrent", systemImage: "plus").labelStyle(.titleAndIcon) }
                    }.buttonStyle(MonoButtonStyle(compact: true)).fixedSize().accessibilityLabel("Add torrent")
                }
            }
            #endif
        }.padding(.vertical, 2).frame(maxWidth: .infinity, alignment: .leading).contentShape(Rectangle())
    }
    /// Queue this copy's episodes as a cloud download, filed under the series' title.
    private func addCloud() async {
        guard !adding, !added, let url = item.shareURL else { return }
        adding = true; addError = nil
        defer { adding = false }
        let name = CloudShareView.libraryName(item.title)
        var body: [String: Any] = ["url": url, "title": name]
        if let key = item.cloud?.key { body["copy"] = key }
        if let season = item.cloud?.season ?? CloudShareView.season(in: item.name) { body["season"] = season }
        if item.cloud?.episodes?.isEmpty == false { body["kind"] = "tv" }
        do {
            let result: CloudAdded = try await store.request("POST", "/v1/cloud/add", body: body, timeout: 45)
            added = true; addNote = result.warning
            if !result.nothingNew { store.notify("Added to downloads", detail: name) }
        } catch { addError = error.localizedDescription }
    }
    private func addMagnet() async {
        guard !adding, !added, !item.magnet.isEmpty else { return }
        adding = true; addError = nil
        defer { adding = false }
        do {
            struct Result: Decodable { let torrentDuplicate: TorrentIdentity? }
            struct TorrentIdentity: Decodable { let name: String? }
            let result: Result = try await store.request("POST", "/v1/torrents", body: ["magnet": item.magnet, "paused": false], timeout: 45)
            added = true
            store.notify(result.torrentDuplicate == nil ? "Torrent added" : "Already in your torrents", detail: item.name, torrentHash: item.infoHash)
            onAdded()
        } catch { addError = error.localizedDescription }
    }
}

private struct TorrentSearchDetail: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) private var dismiss
    let item: TorrentSearchItem
    var embedded = false
    var onAdded: () -> Void = {}
    @State private var detail: TorrentSearchDetails?
    @State private var attachment = 0
    private var release: TorrentSearchDetails.Release? {
        guard let values = detail?.releases, values.indices.contains(attachment) else { return nil }
        return values[attachment]
    }
    private var resolved: TorrentSearchItem { release?.item ?? item }
    private var displayedSize: String { resolved.size }
    @State private var loading = false
    @State private var adding = false
    @State private var paused = false
    @State private var added = false
    @State private var message: String?
    @State private var detailError: String?

    var body: some View {
        List {
            SwiftUI.Section {
                Text(item.title).font(.title2.bold()).searchSelectable
                if !item.edition.isEmpty { Text(item.edition).foregroundStyle(.secondary) }
                Text([item.quality, item.hints.audioLabel, displayedSize].filter { !$0.isEmpty }.joined(separator: " · ")).font(.subheadline).foregroundStyle(.secondary)
                if item.seeders != nil { Label(item.availability, systemImage: "arrow.up.circle.fill").font(.subheadline).foregroundStyle(Theme.accent) }
                Text(item.name).font(.caption).foregroundStyle(.secondary).searchSelectable
                if let releases = detail?.releases, releases.count > 1 {
                    Picker("Release", selection: $attachment) {
                        ForEach(Array(releases.enumerated()), id: \.offset) { index, release in
                            Text(release.item.name).tag(index)
                        }
                    }
                }
                Toggle("Add paused", isOn: $paused).disabled(adding || added)
                Button { Task { await add() } } label: {
                    Label(LocalizedStringKey(added ? "Added to Transmission" : adding ? "Adding…" : "Add torrent"), systemImage: added ? "checkmark.circle.fill" : "plus.circle.fill")
                }.buttonStyle(MonoButtonStyle(prominent: true)).disabled(adding || added)
                if let message { Text(L10n.text(message)).foregroundStyle(.secondary) }
            }
            if !item.hints.summary.isEmpty {
                SwiftUI.Section("Release tags") { Text(item.hints.summary) }
            }
            SwiftUI.Section("Sources") {
                ForEach(item.sources, id: \.url) { source in
                    #if os(tvOS)
                    Text(source.label)
                    #else
                    Link(source.label, destination: URL(string: source.url)!)
                    #endif
                }
            }
            if loading { ProgressView("Loading details…") }
            if let detailError { Text(detailError).foregroundStyle(.secondary) }
            if let detail {
                if !detail.languages.isEmpty {
                    SwiftUI.Section("Source languages") {
                        ForEach(detail.languages, id: \.self) { Text($0) }
                    }
                }
                if !detail.description.isEmpty {
                    #if os(tvOS)
                    SwiftUI.Section("Description") { Text(detail.description).font(.callout) }
                    #else
                    SwiftUI.Section { DisclosureGroup("Description") { Text(detail.description).font(.callout).searchSelectable } }
                    #endif
                }
                if let files = release?.files ?? detail.files {
                    SwiftUI.Section("Files · \(files.count)") {
                        ForEach(Array(files.enumerated()), id: \.offset) { _, file in
                            VStack(alignment: .leading, spacing: 5) {
                                Text(file.name).lineLimit(3)
                                if !file.hints.summary.isEmpty { Text(file.hints.summary).font(.caption).foregroundStyle(.secondary) }
                                if let size = file.sizeBytes { Text(ByteCountFormatter.string(fromByteCount: Int64(clamping: size), countStyle: .file)).font(.caption).foregroundStyle(.secondary) }
                            }
                        }
                    }
                }
            }
        }.searchDetailTitle(embedded: embedded)
        .task {
            guard let source = item.sources.first(where: { $0.provider == "tpb" }) ?? item.sources.first(where: { $0.provider == "nyaa" }) ?? item.sources.first else { return }
            loading = true; defer { loading = false }
            do { detail = try await store.request("POST", "/v1/search/torrents/detail", body: ["provider": source.provider, "id": source.id], timeout: 45) }
            catch { if !Task.isCancelled { detailError = "Couldn’t load source details. You can still add this torrent." } }
        }
    }
    private func add() async {
        guard !adding && !added else { return }; adding = true; message = nil
        defer { adding = false }
        do {
            struct Torrent: Decodable { let name: String?; let hashString: String? }
            struct Added: Decodable { let torrentAdded: Torrent?; let torrentDuplicate: Torrent? }
            var payload: [String: Any] = ["paused": paused]
            if !resolved.magnet.isEmpty {
                payload["magnet"] = resolved.magnet
            } else { message = "This release has no downloadable torrent."; return }
            let result: Added = try await store.request("POST", "/v1/torrents", body: payload, timeout: 45)
            added = true; message = result.torrentDuplicate == nil ? "Torrent added" : "Already in your torrents"
            store.notify(message ?? "Torrent added", detail: item.name, torrentHash: resolved.infoHash)
            #if !os(tvOS)
            onAdded()
            if !embedded { dismiss() }
            #endif
        } catch { message = error.localizedDescription }
    }
}

private extension View {
    @ViewBuilder func searchDetailTitle(embedded: Bool) -> some View {
        if embedded { self } else { self.navigationTitle("Release") }
    }
    @ViewBuilder var searchSelectable: some View {
        #if os(tvOS)
        self
        #else
        self.textSelection(.enabled)
        #endif
    }
}

#if os(macOS)
private struct TorrentTrial: Decodable, Identifiable {
    struct Candidate: Decodable {
        let name: String?
        let hash: String?
        let state: String
        let bytes: UInt64?
        let speed: Double?
        let peers: Int?
        let sending: Int?
        let error: String?
    }
    let id: String
    let state: String
    let candidates: [Candidate]
    let error: String?
    let winner: String?
}
private struct TorrentTrialsView: View {
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) private var dismiss
    @State private var trials: [TorrentTrial] = []
    @State private var error: String?
    @State private var busy = false
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack { Text("Trials").font(.title2.bold()); Spacer(); Button("Close") {dismiss()} }
            Text("Five minutes. About 256 MB per release. The fastest progressing release is kept; other trial downloads are deleted.")
                .font(.callout).foregroundStyle(.secondary)
            if let error {Text(error).foregroundStyle(.orange)}
            ScrollView {
                VStack(spacing: 16) {
                    ForEach(trials) { trial in
                        VStack(alignment: .leading, spacing: 8) {
                            HStack {
                                Text(LocalizedStringKey(stateName(trial.state))).font(.headline)
                                Spacer()
                                if trial.state == "running" || trial.state == "review" {
                                    Button("Cancel trial") {Task {await action(trial,body:["cancel":true])}}.disabled(busy)
                                }
                            }
                            if let error=trial.error {Text(L10n.text(error)).font(.caption).foregroundStyle(.orange)}
                            ForEach(Array(trial.candidates.enumerated()), id: \.offset) { _, candidate in
                                HStack(spacing: 12) {
                                    VStack(alignment: .leading,spacing: 4) {
                                        Text(candidate.name ?? "").lineLimit(2)
                                        Text(LocalizedStringKey(stateName(candidate.state))).font(.caption).foregroundStyle(.secondary)
                                    }.frame(maxWidth:.infinity,alignment:.leading)
                                    VStack(alignment:.trailing,spacing:4) {
                                        Text(ByteCountFormatter.string(fromByteCount:Int64(candidate.speed ?? 0),countStyle:.file)+"/s")
                                        Text(ByteCountFormatter.string(fromByteCount:Int64(clamping:candidate.bytes ?? 0),countStyle:.file)).font(.caption).foregroundStyle(.secondary)
                                    }.monospacedDigit().frame(width:105,alignment:.trailing)
                                    Text("\(candidate.peers ?? 0) connected · \(candidate.sending ?? 0) sending").font(.caption).frame(width:150)
                                    if candidate.hash == trial.winner {Image(systemName:"checkmark.circle.fill").foregroundStyle(.blue)}
                                    else if trial.state == "review", candidate.state != "removed", let hash=candidate.hash {
                                        Button("Keep this release") {Task {await action(trial,body:["winner":hash])}}.disabled(busy)
                                    }
                                }.padding(10).background(Theme.panel,in:RoundedRectangle(cornerRadius:7))
                            }
                        }.padding(12).background(Theme.canvas,in:RoundedRectangle(cornerRadius:8))
                    }
                    if trials.isEmpty {Text("No trials yet").foregroundStyle(.secondary)}
                }
            }
        }.padding(20).frame(minWidth:760,minHeight:460)
            .task {while !Task.isCancelled {await refresh();try? await Task.sleep(for:.seconds(3))}}
    }
    private func refresh() async {
        do {struct Response:Decodable {let trials:[TorrentTrial]};let response:Response=try await store.request("GET","/v1/trials");trials=response.trials}
        catch {self.error=error.localizedDescription}
    }
    private func action(_ trial:TorrentTrial,body:[String:Any]) async {
        busy=true;defer{busy=false}
        do {let _:TorrentTrial=try await store.request("POST","/v1/trials/\(trial.id)",body:body);await refresh()}
        catch {self.error=error.localizedDescription}
    }
    private func stateName(_ state:String)->String {
        ["starting":"Preparing trial","pending":"Preparing trial","running":"Comparing releases","ready":"Comparing releases","paused":"Trial measured","review":"Trial needs attention","finishing":"Keeping best release","complete":"Trial complete","cancelled":"Trial cancelled","removed":"Removed","duplicate":"Already in your torrents","error":"Couldn’t add release"][state] ?? state
    }
}
#endif

// MARK: Missing episodes
/// A show's aired episodes with no file in the library, by season (from the server).
struct MissingEpisodes: Decodable {
    struct Episode: Decodable { let episode: Int; let title: String?; let airdate: String? }
    struct Season: Decodable { let season: Int; let aired: Int; let missing: [Episode] }
    let title: String?
    let originalTitle: String?
    let seasons: [Season]
    var incomplete: [Season] { seasons.filter { !$0.missing.isEmpty } }
}

/// Under a show's seasons: what is missing, and a button that looks for just those episodes.
struct MissingEpisodesBar: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    let title: LibraryTitle
    let season: Int?
    @State private var missing: MissingEpisodes?
    @State private var finding: MissingEpisodes.Season?
    /// The selected season when it is missing episodes; otherwise the first season that is.
    private var target: MissingEpisodes.Season? {
        guard let missing else { return nil }
        return missing.incomplete.first { $0.season == season } ?? missing.incomplete.first
    }
    var body: some View {
        Group {
            if let missing, let target {
                let others = missing.incomplete.filter { $0.season != target.season }
                HStack(alignment: .center, spacing: 12) {
                    Image(systemName: "rectangle.stack.badge.plus").foregroundStyle(.orange)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(L10n.format("Season %d is missing %d of %d episodes", target.season, target.missing.count, target.aired))
                            .font(.subheadline.weight(.semibold))
                        Text(verbatim: (target.missing.prefix(12).map { "E" + String(format: "%02d", $0.episode) } + (target.missing.count > 12 ? ["…"] : [])).joined(separator: " ")
                             + (others.isEmpty ? "" : " · " + L10n.format("Also missing: %@", others.map { L10n.format("Season %d", $0.season) }.joined(separator: ", "))))
                            .font(.caption).monospacedDigit().foregroundStyle(Theme.muted).lineLimit(2)
                    }
                    Spacer(minLength: 8)
                    if others.isEmpty {
                        Button("Find missing episodes") { finding = target }.buttonStyle(MonoButtonStyle(prominent: true, compact: true)).fixedSize()
                    } else {
                        Menu {
                            ForEach(missing.incomplete, id: \.season) { item in
                                Button(L10n.format("Season %d · %d missing", item.season, item.missing.count)) { finding = item }
                            }
                        } label: { Text("Find missing episodes") }.buttonStyle(MonoButtonStyle(prominent: true, compact: true)).fixedSize()
                    }
                }.padding(12).background(Theme.panel, in: RoundedRectangle(cornerRadius: 10))
            }
        }
        .task(id: "\(title.id)|\(title.assets.count)") {
            missing = try? await store.request("GET", "/v1/library/titles/\(title.id)/missing-episodes", timeout: 20)
        }
        .sheet(item: Binding(get: { finding.map { FindingSeason(season: $0) } }, set: { finding = $0?.season })) { item in
            LocalizedInterface { MissingEpisodesSheet(show: title.title, originalTitle: missing?.originalTitle, season: item.season) }.environmentObject(store)
        }
    }
    private struct FindingSeason: Identifiable { let season: MissingEpisodes.Season; var id: Int { season.season } }
}

/// Searches for only a season's missing episodes: one search for the season, then one per
/// episode it did not turn up, and lists the best release of each.
private struct MissingEpisodesSheet: View {
    @EnvironmentObject var store: Store
    let show: String
    var originalTitle: String? = nil
    let season: MissingEpisodes.Season
    @State private var cloudOffer: CloudOffer?
    @AppStorage("torrentSearchMarket") private var market = "world"
    @AppStorage("torrentSearchResolution") private var resolution = "all"
    @State private var found: [Int: [TorrentSearchItem]] = [:]
    @State private var progress: String? = "Searching for the missing episodes…"
    @State private var searched: Set<Int> = []
    private var wanted: Set<Int> { Set(season.missing.map(\.episode)) }
    private var picks: [EpisodePick] {
        EpisodePick.choose(found.values.flatMap { $0 }, season: season.season, only: wanted, ownership: owned)
    }
    var body: some View {
        let picks = picks
        let chosen = Set(picks.map(\.episode))
        EpisodeDownloaderSheet(show: show, season: season.season, picks: picks,
                               notFound: season.missing.map(\.episode).filter { !chosen.contains($0) && (progress == nil || searched.contains($0)) },
                               progress: progress, cloudOffer: cloudOffer)
            .task { await search() }
            .task { await searchCloud() }
    }
    private func owned(_ item: TorrentSearchItem) -> String? {
        store.torrents.contains { !item.infoHash.isEmpty && $0.hashString.caseInsensitiveCompare(item.infoHash) == .orderedSame } ? "Already in your torrents" : nil
    }
    /// Single-episode releases of this show, with seeds, in the chosen resolution.
    private func usable(_ items: [TorrentSearchItem]) -> [TorrentSearchItem] {
        let key = TorrentSearchView.titleKey(show)
        let episodes = items.filter { $0.isSingleEpisode && !$0.magnet.isEmpty && ($0.seeders ?? 0) > 0
            && (resolution == "all" || $0.hints.resolution?.lowercased() == resolution) }
        let named = episodes.filter { TorrentSearchView.titleKey($0.title) == key }
        if !named.isEmpty { return named }
        // Releases often name a show differently; fall back to the search's leading show.
        guard let lead = episodes.first.map({ TorrentSearchView.titleKey($0.title) }) else { return [] }
        return episodes.filter { TorrentSearchView.titleKey($0.title) == lead }
    }
    private func run(episode: Int?) async -> [TorrentSearchItem] {
        var body: [String: Any] = ["query": show, "market": market, "sources": [] as [String], "season": season.season]
        if let episode { body["episode"] = episode }
        let response: TorrentSearchResponse? = try? await store.request("POST", "/v1/search/torrents", body: body, timeout: 30)
        return usable(response?.items ?? []).filter { $0.hints.season ?? 1 == season.season }
    }
    /// A Chinese show is also looked for on Quark Drive, by its Chinese title: the share holding
    /// the most missing episodes of this season is offered with just those files chosen.
    private func searchCloud() async {
        let chinese = [originalTitle, show].compactMap { $0 }.first { $0.range(of: #"\p{Han}"#, options: .regularExpression) != nil }
        guard let query = chinese ?? (market == "china" ? show : nil) else { return }
        guard let found: CloudSearch = try? await store.request("POST", "/v1/cloud/search", body: ["query": query], timeout: 90) else { return }
        // One copy of one share: the one holding the most missing episodes, then the chosen
        // resolution, the sharper picture, surround sound and the native frame rate.
        var options: [(CloudShare, [CloudShare.File], [Int])] = []
        for share in found.shares {
            let keys: [String?] = share.copies.map { $0.map(\.key) } ?? [nil]
            for key in keys {
                let copy = share.copies?.first { $0.key == key }
                let files = share.files.filter { file in
                    (key == nil || file.copy == key) && (file.season ?? copy?.season ?? season.season) == season.season && file.episode.map(wanted.contains) == true
                }
                guard !files.isEmpty else { continue }
                let picture = ["2160p": 3, "1080p": 2, "720p": 1][copy?.resolution ?? ""] ?? 0
                let sound = copy?.audio == nil ? 1 : copy?.audio == "stereo" ? 0 : 2
                options.append((share, files, [Set(files.compactMap(\.episode)).count, resolution != "all" && copy?.resolution == resolution ? 1 : 0, picture, sound,
                                               (copy?.fps ?? 0) >= 49 ? 0 : 1, Int(clamping: (copy?.totalBytes ?? 0) >> 20)]))
            }
        }
        let best = options.max { $0.2.lexicographicallyPrecedes($1.2) }.map { ($0.0, $0.1) }
        guard let (share, files) = best else { return }
        // One file per episode: the largest copy when a share holds several.
        var byEpisode: [Int: CloudShare.File] = [:]
        for file in files { if let episode = file.episode, (byEpisode[episode]?.sizeBytes ?? -1) < (file.sizeBytes ?? 0) { byEpisode[episode] = file } }
        cloudOffer = CloudOffer(share: share, files: byEpisode.keys.sorted().compactMap { byEpisode[$0] }, title: show, season: season.season)
    }
    private func search() async {
        for item in await run(episode: nil) { if let episode = item.hints.episode, wanted.contains(episode) { found[episode, default: []].append(item) } }
        // Each episode the season search missed gets a search of its own; the server caches and
        // coalesces them. Bounded so a long-running show cannot start dozens of searches.
        let remaining = season.missing.map(\.episode).filter { found[$0] == nil }
        for episode in remaining.prefix(12) {
            guard !Task.isCancelled else { return }
            progress = L10n.format("Searching for episode %d…", episode)
            for item in await run(episode: episode) where item.hints.episode == episode { found[episode, default: []].append(item) }
            searched.insert(episode)
        }
        searched.formUnion(remaining)
        progress = nil
    }
}

// MARK: Cloud shares (Quark Drive)
/// A cloud-drive share's contents, as the server found them when it checked the share.
struct CloudShare: Decodable {
    struct File: Decodable, Identifiable {
        let path: String; let name: String; let sizeBytes: Int64?; let season: Int?; let episode: Int?
        /// Which of the share's copies the video belongs to.
        var copy: String? = nil
        var width: Int? = nil
        var height: Int? = nil
        /// The picture size the drive measured for this video.
        var resolution: String? {
            let (w, h) = (width ?? 0, height ?? 0)
            guard w > 0 || h > 0 else { return nil }
            return w >= 3000 || h >= 2000 ? "2160p" : w >= 1800 || h >= 1000 ? "1080p" : w >= 1200 || h >= 700 ? "720p" : "480p"
        }
        var id: String { path }
        var label: String {
            switch (season, episode) {
            case let (season?, episode?): return L10n.format("Season %d Ep %d", season, episode)
            case let (nil, episode?): return L10n.format("Ep %d", episode)
            default: return name
            }
        }
    }
    let url: String
    let title: String?
    let posted: String?
    let videoCount: Int?
    let totalBytes: Int64?
    let files: [File]
    var copies: [CloudCopy]? = nil
}
struct CloudSearch: Decodable { let shares: [CloudShare] }
/// The server's answer to a request to download from a share.
struct CloudAdded: Decodable {
    var jobId: String? = nil
    /// A download of these same videos was already waiting or running.
    var alreadyQueued: Bool? = nil
    /// How many of the videos asked for are in the library already, exactly the same, out of how many.
    var alreadyHave: Int? = nil
    var total: Int? = nil
    /// The videos joined a download of this show that was already waiting.
    var merged: Bool? = nil
    /// Nothing new was queued.
    var nothingNew: Bool { alreadyQueued == true || jobId == nil }
    /// What to tell the member, when it is not simply "added".
    var warning: String? {
        if alreadyQueued == true { return L10n.text("Already in the download queue.") }
        if merged == true { return L10n.text("Added to the download already waiting for this show.") }
        guard let alreadyHave, alreadyHave > 0, let total else { return nil }
        return alreadyHave >= total ? L10n.format("Already in your library: all %d videos. Nothing to download.", total) : L10n.format("%d of %d videos are already in your library; the rest will download.", alreadyHave, total)
    }
}

/// One share: its videos with checkboxes, the library name to file them under, and Add.
struct CloudShareView: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) private var dismiss
    let item: TorrentSearchItem
    let query: String
    var embedded = false
    @State private var share: CloudShare?
    @State private var loading = true
    @State private var chosen: Set<String> = []
    @State private var name = ""
    @State private var adding = false
    @State private var message: String?
    @State private var added = false
    @State private var measured: CloudMeasured?
    @State private var measuring = false
    /// This copy with its tracks as read just now, when the search had not read them yet.
    private var copy: CloudCopy? {
        guard var copy = item.cloud else { return nil }
        if let measured { copy.measured = measured }
        return copy
    }
    /// The videos of this result's copy; a share may hold the series several times over.
    private var files: [CloudShare.File] {
        guard let share else { return [] }
        guard let key = item.cloud?.key else { return share.files }
        return share.files.filter { $0.copy == key }
    }
    private var chosenBytes: Int64 { files.filter { chosen.contains($0.path) }.reduce(0) { $0 + ($1.sizeBytes ?? 0) } }
    var body: some View {
        List {
            SwiftUI.Section {
                Text(item.title).font(.title2.bold()).textSelectionIfAvailable()
                if let cloud = copy {
                    Text(verbatim: cloud.episodesLine).font(.headline).monospacedDigit()
                        .foregroundStyle(cloud.complete == true ? Theme.accent : cloud.aired == nil ? Color.primary : Color.orange)
                    if !cloud.qualityLine.isEmpty { Text(verbatim: cloud.qualityLine).font(.subheadline).foregroundStyle(.secondary) }
                    if let tracks = cloud.measured {
                        // Every sound track, the picture as encoded, and any subtitles inside the file.
                        LabeledContent("Sound") {
                            VStack(alignment: .trailing, spacing: 2) {
                                ForEach(Array((tracks.audio ?? []).enumerated()), id: \.offset) { _, track in
                                    Text(verbatim: [track.label, track.language.map(CloudMeasured.language) ?? ""].filter { !$0.isEmpty }.joined(separator: " · "))
                                }
                            }
                        }
                        if let video = tracks.video {
                            LabeledContent("Picture") {
                                Text(verbatim: [video.width.flatMap { w in video.height.map { "\(w)×\($0)" } } ?? "", video.codec ?? "", video.hdr ?? ""].filter { !$0.isEmpty }.joined(separator: " · "))
                            }
                        }
                        LabeledContent("Subtitles") {
                            Text(verbatim: (tracks.subtitles ?? []).isEmpty ? L10n.text("None in the file") : (tracks.subtitles ?? []).map(CloudMeasured.language).joined(separator: " · "))
                        }
                    } else if measuring {
                        HStack(spacing: 10) { ProgressView().controlSize(.small); Text("Checking sound and subtitles…").foregroundStyle(.secondary) }
                    }
                }
                if !item.cloudSummary.isEmpty { Text(item.cloudSummary).font(.subheadline).foregroundStyle(.secondary) }
                Text(item.name).font(.caption).foregroundStyle(.secondary).textSelectionIfAvailable()
                LabeledContent("Save as") {
                    TextField("Title", text: $name).labelsHidden().multilineTextAlignment(.trailing).autocorrectionDisabled()
                }
                Button { Task { await add() } } label: {
                    Label(LocalizedStringKey(added ? "Added to downloads" : adding ? "Adding…" : chosen.isEmpty && share != nil ? "Choose videos to add" : "Add"),
                          systemImage: added ? "checkmark.circle.fill" : "icloud.and.arrow.down.fill")
                }.buttonStyle(MonoButtonStyle(prominent: true))
                    .disabled(adding || added || (share != nil && chosen.isEmpty) || name.trimmingCharacters(in: .whitespaces).isEmpty)
                if let message { Text(LocalizedStringKey(message)).font(.caption).foregroundStyle(.secondary) }
            } footer: {
                Text("Videos are saved to your Quark Drive, downloaded into your library, then removed from the drive.")
            }
            if loading {
                HStack(spacing: 10) { ProgressView().controlSize(.small); Text("Checking the share’s files…").foregroundStyle(.secondary) }
            } else if share != nil {
                SwiftUI.Section {
                    ForEach(files) { file in
                        Button { if chosen.contains(file.path) { chosen.remove(file.path) } else { chosen.insert(file.path) } } label: {
                            HStack(spacing: 12) {
                                Image(systemName: chosen.contains(file.path) ? "checkmark.circle.fill" : "circle")
                                    .foregroundStyle(chosen.contains(file.path) ? Theme.accent : Theme.muted).font(.title3)
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(verbatim: file.label).font(.headline)
                                    Text(verbatim: file.name).font(.caption).foregroundStyle(.secondary).lineLimit(2)
                                }
                                Spacer(minLength: 8)
                                // An episode in another quality than the rest of its set is called out.
                                if let size = file.resolution, let usual = item.cloud?.resolution, size != usual {
                                    Text(verbatim: size).font(.caption.weight(.semibold)).foregroundStyle(.orange)
                                }
                                if let size = file.sizeBytes { Text(ByteCountFormatter.string(fromByteCount: size, countStyle: .file)).font(.caption).monospacedDigit().foregroundStyle(.secondary) }
                            }.contentShape(Rectangle())
                        }.buttonStyle(.plain).disabled(added)
                    }
                } header: {
                    HStack {
                        Text(L10n.format("%d of %d videos · %@", chosen.count, files.count, ByteCountFormatter.string(fromByteCount: chosenBytes, countStyle: .file)))
                            .monospacedDigit()
                        Spacer()
                        Button(LocalizedStringKey(chosen.count == files.count ? "Select none" : "Select all")) {
                            chosen = chosen.count == files.count ? [] : Set(files.map(\.path))
                        }.buttonStyle(.borderless).disabled(added)
                    }.textCase(nil)
                }
            } else {
                Text("This share’s file list is no longer available. Add brings in every video it holds.").foregroundStyle(.secondary)
            }
        }
        .navigationTitle(embedded ? "" : L10n.text("Quark Drive"))
        .task(id: item.id) {
            name = Self.libraryName(item.title)
            defer { loading = false }
            let found: CloudSearch? = try? await store.request("POST", "/v1/cloud/search", body: ["query": query], timeout: 90)
            share = found?.shares.first { $0.url == item.shareURL }
            // Trailers and extras are listed but not chosen for you.
            chosen = Set(files.filter { $0.episode != nil || item.cloud?.episodes?.isEmpty != false }.map(\.path))
        }
        .task(id: item.id) {
            // The first episode's tracks are read from the drive; a few seconds, once per copy.
            measured = nil
            guard item.cloud?.measured == nil, let url = item.shareURL, let key = item.cloud?.key else { return }
            measuring = true
            defer { measuring = false }
            let found: CloudMeasured? = try? await store.request("POST", "/v1/cloud/probe", body: ["url": url, "copy": key], timeout: 60)
            if !Task.isCancelled { measured = found }
        }
    }
    /// A share's title without its season, edition and quality ("庆余年 第一季 特别版 4K" → "庆余年"),
    /// since it names the library folder.
    static func libraryName(_ title: String) -> String {
        let trimmed = title.replacingOccurrences(of: #"\s*(?:第[一二三四五六七八九十百\d]+[季部]|[Ss]\d{1,2}\b|Season\s*\d|全\d+集|\d{3,4}[pP]\b|4[Kk]\b|特别版|未删减|完结|更新至|国剧|全集|合集|完整版|蓝光|高清).*$"#, with: "", options: .regularExpression)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? title : trimmed
    }
    /// The season a share's title names ("第二季", "S02", "Season 2"), for files that do not say.
    static func season(in title: String) -> Int? {
        if let range = title.range(of: #"第([一二三四五六七八九十\d]{1,3})季"#, options: .regularExpression) {
            let numeral = title[range].dropFirst().dropLast()
            if let number = Int(numeral) { return number }
            let digits: [Character: Int] = ["一": 1, "二": 2, "三": 3, "四": 4, "五": 5, "六": 6, "七": 7, "八": 8, "九": 9]
            if numeral == "十" { return 10 }
            if numeral.hasPrefix("十"), let unit = numeral.last.flatMap({ digits[$0] }) { return 10 + unit }
            if numeral.count == 1, let unit = numeral.first.flatMap({ digits[$0] }) { return unit }
            return nil
        }
        if let range = title.range(of: #"(?i)\b(?:S|Season\s*)(\d{1,2})\b"#, options: .regularExpression) {
            return Int(title[range].filter(\.isNumber))
        }
        return nil
    }
    private func add() async {
        guard let url = item.shareURL, !adding else { return }
        adding = true; message = nil
        defer { adding = false }
        var body: [String: Any] = ["url": url, "title": name.trimmingCharacters(in: .whitespaces)]
        if share != nil {
            body["files"] = files.filter { chosen.contains($0.path) }.map(\.path)
            let seasons = Set(files.filter { chosen.contains($0.path) }.compactMap(\.season))
            if seasons.count == 1 { body["season"] = seasons.first }
            else if seasons.isEmpty, let season = item.cloud?.season ?? Self.season(in: item.name) { body["season"] = season }
            if files.contains(where: { $0.episode != nil }) { body["kind"] = "tv" }
        } else if let key = item.cloud?.key {
            // Without the file list the server still takes this copy alone, not the whole share.
            body["copy"] = key
            if let season = item.cloud?.season { body["season"] = season }
        }
        do {
            let result: CloudAdded = try await store.request("POST", "/v1/cloud/add", body: body, timeout: 45)
            added = !result.nothingNew; message = result.warning ?? "Follow its progress in Downloading."
            if !result.nothingNew { store.notify("Added to downloads", detail: name) }
        } catch { message = error.localizedDescription }
    }
}

/// Cloud-drive downloads in progress, shown with the torrents being downloaded.
struct CloudJob: Decodable, Identifiable {
    let id: String
    let title: String
    let state: String
    var files: Int? = nil
    var filesDone: Int? = nil
    var doneBytes: Int64? = nil
    var totalBytes: Int64? = nil
    var error: String? = nil
    /// Bytes a second, seconds left, and the video being fetched, while downloading.
    var rate: Int64? = nil
    var eta: Int? = nil
    var current: String? = nil
    /// When it was added, in seconds since 1970.
    var created: Double? = nil
    /// Episodes that took the place of a lesser copy, and ones skipped because the library's copy was as good.
    var replaced: Int? = nil
    var discarded: Int? = nil
    /// Its place in the line while waiting: 1 runs next.
    var position: Int? = nil
    /// The show's English title, when the library or the metadata service knows it.
    var english: String? = nil
    /// "Pursuit of Jade · 逐玉": both names, so the row can be read either way.
    var displayTitle: String { english.map { $0 + " · " + title } ?? title }
    var outcomeLine: String {
        [(replaced ?? 0) > 0 ? L10n.format("Replaced with better: %d", replaced ?? 0) : "", (discarded ?? 0) > 0 ? L10n.format("Already had as good: %d", discarded ?? 0) : ""]
            .filter { !$0.isEmpty }.joined(separator: " · ")
    }
    var fraction: Double { (totalBytes ?? 0) > 0 ? Double(doneBytes ?? 0) / Double(totalBytes ?? 1) : 0 }
    var isActive: Bool { ["queued", "saving", "downloading", "retrying"].contains(state) }
    var stateLabel: String {
        switch state {
        case "retrying": return L10n.text("Retrying shortly…")
        case "queued": return L10n.text("Waiting")
        case "saving": return L10n.text("Saving to Quark Drive…")
        case "downloading": return L10n.text("Downloading")
        case "done": return L10n.text("Downloaded")
        case "failed": return L10n.text("Failed")
        default: return state
        }
    }
}
struct CloudJobsSection: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    @State private var jobs: [CloudJob] = []
    /// The gap its container leaves between sections, taken back while there is nothing to show.
    var sectionSpacing: CGFloat = 0
    private struct JobList: Decodable { let jobs: [CloudJob] }
    var body: some View {
        // Always a view, even with no downloads: a task on an empty group never starts,
        // and then the downloads are never asked for.
        VStack(alignment: .leading, spacing: 10) {
            if !jobs.isEmpty {
                Label("Quark Drive", systemImage: "icloud.and.arrow.down").font(.caption.weight(.semibold)).tracking(1.4).foregroundStyle(Theme.muted)
                ForEach(jobs) { job in CloudJobRow(job: job, changed: reload) }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.bottom, jobs.isEmpty ? -sectionSpacing : 0)
        .task(id: store.server) {
            while !Task.isCancelled {
                if let list: JobList = try? await store.request("GET", "/v1/cloud/jobs", timeout: 15) { jobs = shown(list.jobs) }
                let active = jobs.contains(where: \.isActive)
                try? await Task.sleep(for: .seconds(active ? 3 : 15))
            }
        }
    }
    private func reload() {
        Task { if let list: JobList = try? await store.request("GET", "/v1/cloud/jobs", timeout: 15) { jobs = shown(list.jobs) } }
    }
    /// A finished download is in the library; this section lists only what is still coming or has failed.
    private func shown(_ all: [CloudJob]) -> [CloudJob] { all.filter { $0.state != "done" } }
}
/// One cloud-drive download: its title in the downloads' colour, progress, speed, time
/// left, the video being fetched, and Cancel, Retry or Dismiss. Sits among the torrents.
struct CloudJobRow: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    let job: CloudJob
    /// Called after an action, so the list can be fetched again at once.
    var changed: () -> Void = {}
    var body: some View { row(job) }
    private func row(_ job: CloudJob) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline) {
                // Blue titles are cloud downloads, green ones torrents; nothing else is coloured.
                Text(verbatim: job.displayTitle).font(.headline).foregroundStyle(Theme.cloud).lineLimit(2).textSelectionIfAvailable()
                Spacer(minLength: 8)
                if job.state == "failed" || job.state == "retrying" {
                    Button("Retry") { act("POST", "/v1/cloud/jobs/\(job.id)/retry") }.buttonStyle(MonoButtonStyle(compact: true)).fixedSize()
                }
                if job.state == "queued", let position = job.position, position > 1 {
                    // Downloads run in the order added; this one jumps the line.
                    Button("Download next") { act("POST", "/v1/cloud/jobs/\(job.id)/next") }.buttonStyle(MonoButtonStyle(compact: true)).fixedSize()
                }
                if job.isActive {
                    // Stops the download and clears what it had fetched of unfinished videos.
                    Button("Cancel") { act("DELETE", "/v1/cloud/jobs/\(job.id)") }.buttonStyle(MonoButtonStyle(compact: true)).fixedSize()
                }
                if ["done", "failed"].contains(job.state) {
                    Button("Dismiss") { act("DELETE", "/v1/cloud/jobs/\(job.id)") }.buttonStyle(MonoButtonStyle(compact: true)).fixedSize()
                }
            }
            HStack(spacing: 8) {
                Text(verbatim: job.state == "queued" && job.position != nil ? L10n.format("Waiting · %d in line", job.position ?? 1) : job.stateLabel).foregroundStyle(job.state == "failed" || job.state == "retrying" ? .orange : Theme.accent)
                if let files = job.files, files > 1 { Text(L10n.format("%d of %d videos", job.filesDone ?? 0, files)).foregroundStyle(Theme.muted) }
                if let total = job.totalBytes, total > 0 {
                    Text(L10n.format("%@ of %@", ByteCountFormatter.string(fromByteCount: job.doneBytes ?? 0, countStyle: .file), ByteCountFormatter.string(fromByteCount: total, countStyle: .file)))
                        .foregroundStyle(Theme.muted)
                }
                if let rate = job.rate, rate > 0 { Text(verbatim: Torrbaby.rate(rate)).foregroundStyle(Theme.muted) }
                if let eta = job.eta, job.state == "downloading" {
                    let minutes = max(1, (eta + 30) / 60)
                    Text(minutes >= 60 ? L10n.format("%dh %dm left", minutes / 60, minutes % 60) : L10n.format("%dm left", minutes)).foregroundStyle(Theme.muted)
                }
            }.font(.caption).monospacedDigit()
            if let total = job.totalBytes, total > 0, job.state != "done" {
                // The same gauge as a torrent's, so the two read alike in one list.
                #if os(tvOS)
                ProgressView(value: Double(job.doneBytes ?? 0), total: Double(total)).tint(job.state == "failed" ? .orange : Theme.accent)
                #else
                TorrentProgressGauge(value: job.fraction)
                #endif
            }
            if !job.outcomeLine.isEmpty { Text(verbatim: job.outcomeLine).font(.caption).foregroundStyle(.secondary) }
            if let current = job.current, !current.isEmpty { Text(verbatim: current).font(.caption).foregroundStyle(.secondary).lineLimit(1).textSelectionIfAvailable() }
            if let error = job.error, !error.isEmpty { Text(verbatim: error).font(.caption).foregroundStyle(.orange) }
        }.textSelectionIfAvailable()   // every line of the card can be selected and copied
            .padding(12).background(Theme.panel, in: RoundedRectangle(cornerRadius: 10))
            #if !os(tvOS)
            .contextMenu { Button { Clipboard.copy(job.title) } label: { Label("Copy name", systemImage: "doc.on.doc") } }
            #endif
    }
    private func act(_ method: String, _ path: String) {
        Task {
            struct Ignored: Decodable {}
            do { let _: Ignored = try await store.request(method, path, timeout: 20) } catch { store.actionError = error.localizedDescription }
            changed()
        }
    }
}

extension View {
    @ViewBuilder func textSelectionIfAvailable() -> some View {
        #if os(tvOS)
        self
        #else
        self.textSelection(.enabled)
        #endif
    }
}

/// Missing episodes available in one Quark Drive share.
struct CloudOffer {
    let share: CloudShare
    let files: [CloudShare.File]
    let title: String
    let season: Int
}
private struct CloudOfferSection: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    let offer: CloudOffer
    @State private var adding = false
    @State private var added = false
    @State private var failure: String?
    var body: some View {
        SwiftUI.Section {
            HStack(spacing: 12) {
                VStack(alignment: .leading, spacing: 3) {
                    Text(L10n.format("%d missing episodes on Quark Drive", offer.files.count)).font(.headline)
                    Text(verbatim: offer.files.compactMap(\.episode).map { "E" + String(format: "%02d", $0) }.joined(separator: " ")).font(.caption).monospacedDigit()
                    Text(verbatim: offer.share.title ?? "").font(.caption).foregroundStyle(Theme.muted).lineLimit(2)
                    if let failure { Text(failure).font(.caption).foregroundStyle(.orange) }
                }
                Spacer(minLength: 8)
                if added { Label("Added", systemImage: "checkmark.circle.fill").font(.caption.weight(.semibold)) }
                else if adding { ProgressView().controlSize(.small) }
                else {
                    Button { Task { await add() } } label: { Label("Add", systemImage: "icloud.and.arrow.down") }
                        .buttonStyle(MonoButtonStyle(prominent: true, compact: true)).fixedSize()
                }
            }.padding(.vertical, 4)
        }
    }
    private func add() async {
        adding = true; failure = nil
        defer { adding = false }
        do {
            struct Job: Decodable { let jobId: String? }
            let _: Job = try await store.request("POST", "/v1/cloud/add", body: ["url": offer.share.url, "title": offer.title, "season": offer.season, "kind": "tv",
                                                                                "files": offer.files.map(\.path)], timeout: 45)
            added = true
            store.notify("Added to downloads", detail: offer.title)
        } catch { failure = error.localizedDescription }
    }
}

/// Free space on the server's media drive (each drive, when the library spans several).
struct MediaDriveSpace: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    var compact = false
    var body: some View {
        if let storage = store.status?.storage, !storage.volumes.isEmpty {
            VStack(alignment: .leading, spacing: 6) {
                ForEach(storage.volumes, id: \.name) { volume in
                    HStack(spacing: 10) {
                        Image(systemName: volume.available ? "externaldrive.fill" : "externaldrive.badge.exclamationmark")
                            .foregroundStyle(!volume.available || low(volume) ? .orange : Theme.muted)
                        if volume.available {
                            Text(L10n.format("%@ free of %@", bytes(volume.freeBytes), bytes(volume.totalBytes)))
                                .font(.caption.weight(.semibold)).monospacedDigit()
                            if !compact {
                                ProgressView(value: Double(volume.totalBytes - volume.freeBytes), total: Double(max(volume.totalBytes, 1)))
                                    .tint(low(volume) ? .orange : Theme.accent).frame(maxWidth: 160)
                            }
                        } else {
                            Text("Drive not connected").font(.caption.weight(.semibold)).foregroundStyle(.orange)
                        }
                        Text(verbatim: volume.name).font(.caption).foregroundStyle(Theme.muted).lineLimit(1)
                    }
                }
            }.accessibilityElement(children: .combine)
        }
    }
    /// Under 5% or 50 GB left: time to make room.
    private func low(_ volume: MediaStorage.Volume) -> Bool {
        volume.freeBytes < 50_000_000_000 || volume.freeBytes * 20 < volume.totalBytes
    }
    private func bytes(_ value: Int64) -> String { ByteCountFormatter.string(fromByteCount: value, countStyle: .file) }
}

extension View {
    /// A pointing hand over a control in a list, where the cursor otherwise stays as it was.
    @ViewBuilder func pointingHandOnHover() -> some View {
        #if os(macOS)
        onHover { inside in if inside { NSCursor.pointingHand.push() } else { NSCursor.pop() } }
        #else
        self
        #endif
    }
}
