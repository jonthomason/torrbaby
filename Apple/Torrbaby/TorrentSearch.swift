import SwiftUI

struct TorrentSearchSource: Decodable {
    let provider: String
    let id: String
    let url: String
    var label: String { Self.label(provider) }
    static func label(_ provider: String) -> String { ["1lou":"BT之家", "tpb":"The Pirate Bay", "csv":"Torrent CSV", "nyaa":"Nyaa", "dmhy":"动漫花园", "mikan":"蜜柑计划"][provider] ?? provider }
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
    let languages: [String]
    var summary: String { ([resolution, videoCodec, releaseSource].compactMap { $0 } + audio + languages).joined(separator: " · ") }
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
    var sourceKey: String { sources.map { "\($0.provider):\($0.id)" }.joined(separator: ",") }
    var id: String { infoHash.isEmpty ? sourceKey : infoHash }
    var title: String {
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
    var isCompleteSeason: Bool { hints.episodeCount != nil || (hints.season != nil && hints.episode == nil) }
    var isSingleEpisode: Bool { hints.episode != nil && hints.episodeEnd == nil && hints.episodeCount == nil }
    func covers(season wanted: Int) -> Bool {
        guard let season = hints.season else { return false }
        return (season...max(season, hints.seasonEnd ?? season)).contains(wanted)
    }
    func covers(episode wanted: Int) -> Bool {
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
        return seeders == 0 ? "No reported seeds" : "\(seeders.formatted()) \(seeders == 1 ? "seed" : "seeds")"
    }
    var size: String { sizeBytes.map { ByteCountFormatter.string(fromByteCount: Int64(clamping: $0), countStyle: .file) } ?? sizeLabel ?? "" }
}
private struct TorrentSearchResponse: Decodable {
    struct Provider: Decodable { let provider: String; let count: Int; let error: String? }
    var items: [TorrentSearchItem]
    let providers: [Provider]
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
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    @State private var query: String
    @State private var year: Int?
    @State private var season: Int?
    @State private var episode: Int?
    init(initialQuery: String = "", year: Int? = nil, season: Int? = nil, episode: Int? = nil) {
        _query = State(initialValue: initialQuery); _year = State(initialValue: year)
        _season = State(initialValue: season); _episode = State(initialValue: episode)
    }
    @AppStorage("torrentSearchMarket") private var market = "world"
    @AppStorage("torrentSearchResolution") private var resolution = "all"
    @State private var source = "all"
    @State private var scopeFilter = "all"
    @State private var seasonFilter: Int?
    @State private var episodeFilter: Int?
    @AppStorage("torrentSearchSort") private var sortOrder = "match"
    @State private var confirmBatch = false
    @State private var batchAdding = false
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
    @State private var querySelection: TextSelection?
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
    private static func titleKey(_ value: String) -> String {
        value.lowercased().unicodeScalars.filter { CharacterSet.alphanumerics.contains($0) }.map(String.init).joined()
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
    /// The best-seeded single-episode release of each episode the user does not have yet.
    /// A search can return several shows; the batch follows the best-matching one.
    private var batchCandidates: [TorrentSearchItem] {
        let episodes = resolutionItems.filter { $0.isSingleEpisode && !$0.magnet.isEmpty && ($0.seeders ?? 0) > 0 }
        guard let lead = episodes.first.map({ Self.titleKey($0.title) }) else { return [] }
        return episodes.filter { Self.titleKey($0.title) == lead }
    }
    private var batchSeason: Int? {
        if let seasonFilter { return seasonFilter }
        let seasons = Set(batchCandidates.map { $0.hints.season ?? 1 })
        return seasons.count == 1 ? seasons.first : nil
    }
    private var batchPicks: [TorrentSearchItem] {
        guard let season = batchSeason else { return [] }
        var best: [Int: TorrentSearchItem] = [:]
        for item in batchCandidates where (item.hints.season ?? 1) == season {
            guard let episode = item.hints.episode, episodeFilter == nil || episode == episodeFilter, ownership(item) == nil else { continue }
            if (best[episode]?.seeders ?? 0) < (item.seeders ?? 0) { best[episode] = item }
        }
        // An episode the library already has is skipped even when this particular release is new.
        return best.keys.sorted().compactMap { best[$0] }
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
                        } else {
                            ContentUnavailableView("No releases found", systemImage: "magnifyingglass", description: Text("Try a shorter title or another source."))
                        }
                    } else if usesColumns(geometry.size.width) && !visibleItems.allSatisfy({ $0.sources.contains { $0.provider == "1lou" } }) {
                        HStack(spacing: 0) {
                            results(visibleItems, inline: true)
                                .frame(width: min(440, max(280, geometry.size.width * 0.43)))
                            Divider()
                            if let selectedItem {
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
        .onChange(of: query) { _, _ in
            if prepareNextSearch {
                year = nil; season = nil; episode = nil
                prepareNextSearch = false
            }
        }
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
    private var searchHeader: some View {
        VStack(alignment: .leading, spacing: 12) {
            Picker("Search region", selection: $market) {
                Text("World").tag("world")
                Text("China").tag("china")
            }.pickerStyle(.segmented).labelsHidden().frame(maxWidth: 320).frame(maxWidth: .infinity, alignment: .leading)
                .onChange(of: market) {
                    work?.cancel(); searching = false; response = nil; selectedID = nil; error = nil; source = "all"
                    if query.trimmingCharacters(in: .whitespacesAndNewlines).count >= 2 { search() }
                }
            HStack(spacing: 12) {
                Image(systemName: "magnifyingglass").foregroundStyle(Theme.accent)
                searchField
                    .focused($searchFocused)
                    .onSubmit(search).autocorrectionDisabled().textFieldStyle(.plain)
                Button(action: search) { Text(LocalizedStringKey(searching ? "Searching…" : "Search")) }
                    .buttonStyle(MonoButtonStyle(prominent: true))
                    .disabled(query.trimmingCharacters(in: .whitespacesAndNewlines).count < 2 || searching)
            }.padding(12).background(Theme.panel, in: RoundedRectangle(cornerRadius: 10))
            ViewThatFits(in: .horizontal) {
                HStack { sourcePicker; resolutionPicker; Spacer(); searchStatus }
                VStack(alignment: .leading, spacing: 6) {
                    HStack { sourcePicker.labelsHidden(); Spacer(); resolutionPicker.labelsHidden() }
                    searchStatus
                }
            }
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
                    if market == "china" {
                        Text("BT之家").tag("1lou")
                        Text("动漫花园").tag("dmhy")
                        Text("蜜柑计划").tag("mikan")
                    } else {
                        Text("The Pirate Bay").tag("tpb")
                        Text("Torrent CSV").tag("csv")
                        Text("Nyaa").tag("nyaa")
                    }
                }
                #if os(tvOS)
                .pickerStyle(.segmented)
                #else
                .pickerStyle(.menu).fixedSize()
                #endif
    }
    private var resolutionPicker: some View {
                Picker("Resolution", selection: $resolution) {
                    Text("All resolutions").tag("all")
                    Text(verbatim:"2160p").tag("2160p")
                    Text(verbatim:"1080p").tag("1080p")
                    Text(verbatim:"720p").tag("720p")
                }.pickerStyle(.menu).fixedSize()
                    .onChange(of: resolution) { selectedID = visibleItems.first?.id }
    }
    @ViewBuilder private var refinements: some View {
        let picks = batchPicks
        ViewThatFits(in: .horizontal) {
            HStack { scopePicker; seasonPicker; episodePicker; sortPicker; Spacer(); batchButton(picks) }
            VStack(alignment: .leading, spacing: 6) {
                HStack { scopePicker.labelsHidden(); seasonPicker.labelsHidden(); episodePicker.labelsHidden(); Spacer() }
                HStack { sortPicker.labelsHidden(); Spacer(); batchButton(picks) }
            }
        }
        .confirmationDialog(L10n.format("Add %d episodes?", picks.count), isPresented: $confirmBatch, titleVisibility: .visible) {
            Button("Add") { Task { await addBatch(picks) } }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Adds the most-seeded release of each episode that isn’t already in your library or torrents.")
        }
    }
    private var scopePicker: some View {
        Picker("Show", selection: $scopeFilter) {
            Text("All releases").tag("all")
            Text("Complete seasons").tag("seasons")
            Text("Single episodes").tag("episodes")
        }.pickerStyle(.menu).fixedSize()
            .onChange(of: scopeFilter) { selectedID = visibleItems.first?.id }
    }
    @ViewBuilder private var seasonPicker: some View {
        if availableSeasons.count > 1 {
            Picker("Season", selection: $seasonFilter) {
                Text("All seasons").tag(Int?.none)
                ForEach(availableSeasons, id: \.self) { Text("Season \($0)").tag(Int?.some($0)) }
            }.pickerStyle(.menu).fixedSize()
                .onChange(of: seasonFilter) { episodeFilter = nil; selectedID = visibleItems.first?.id }
        }
    }
    @ViewBuilder private var episodePicker: some View {
        if availableEpisodes.count > 1 {
            Picker("Episode", selection: $episodeFilter) {
                Text("All episodes").tag(Int?.none)
                ForEach(availableEpisodes, id: \.self) { Text("Episode \($0)").tag(Int?.some($0)) }
            }.pickerStyle(.menu).fixedSize()
                .onChange(of: episodeFilter) { selectedID = visibleItems.first?.id }
        }
    }
    private var sortPicker: some View {
        Picker("Sort", selection: $sortOrder) {
            Text("Best match").tag("match")
            Text("Episode order").tag("episode")
            Text("Most seeds").tag("seeds")
        }.pickerStyle(.menu).fixedSize()
    }
    @ViewBuilder private func batchButton(_ picks: [TorrentSearchItem]) -> some View {
        if picks.count > 1 {
            if batchAdding { ProgressView().controlSize(.small) }
            else { Button(L10n.format("Add best of %d episodes", picks.count)) { confirmBatch = true } }
        }
    }
    private func addBatch(_ picks: [TorrentSearchItem]) async {
        guard !batchAdding else { return }
        batchAdding = true; error = nil
        defer { batchAdding = false }
        var added = 0
        struct Added: Decodable {}
        for item in picks {
            do {
                let _: Added = try await store.request("POST", "/v1/torrents", body: ["magnet": item.magnet, "paused": false], timeout: 45)
                added += 1
            } catch { self.error = error.localizedDescription; break }
        }
        if added > 0 { store.notify(L10n.format("%d torrents added", added), detail: picks.first?.title ?? "") }
    }
    @ViewBuilder private var searchStatus: some View {
                if response != nil { Text("\(visibleItems.count) releases").font(.caption).foregroundStyle(.secondary) }
                if searching { ProgressView().controlSize(.small) }
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
            if item.sources.contains(where: { $0.provider == "1lou" }) {
                TorrentSearchRow(item: item, onAdded: { prepareNextSearch = true; focusNextSearch() }, ownership: ownership(item), artworkYear: artworkYear(item))
            } else if inline {
                Button { selectedID = item.id } label: { TorrentSearchRow(item: item, ownership: ownership(item), artworkYear: artworkYear(item)) }
                    .buttonStyle(.plain)
                    .listRowBackground(selectedID == item.id ? Theme.accent.opacity(0.13) : Color.clear)
                    .accessibilityAddTraits(selectedID == item.id ? .isSelected : [])
            } else {
                NavigationLink { TorrentSearchDetail(item: item, onAdded: { prepareNextSearch = true }).id(item.id) } label: { TorrentSearchRow(item: item, ownership: ownership(item), artworkYear: artworkYear(item)) }
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
    @ViewBuilder private var searchField: some View {
        #if os(tvOS)
        TextField("Movie, show, or release", text: $query)
        #else
        TextField("Movie, show, or release", text: $query, selection: $querySelection)
        #endif
    }
    private func focusNextSearch() {
        #if !os(tvOS)
        Task { @MainActor in
            // Allow the phone's detail navigation to finish before opening its keyboard.
            await Task.yield()
            searchFocused = true
            querySelection = TextSelection(range: query.startIndex..<query.endIndex)
        }
        #endif
    }
    private func search() {
        searchFocused = false
        guard query.trimmingCharacters(in: .whitespacesAndNewlines).count >= 2 else { return }
        work?.cancel(); searching = true; error = nil
        var body: [String: Any] = ["query": query, "market": market, "sources": source == "all" ? [] : [source]]
        if let year { body["year"] = year }; if let season { body["season"] = season }; if let episode { body["episode"] = episode }
        work = Task { @MainActor in
            do {
                let value: TorrentSearchResponse = try await store.request("POST", "/v1/search/torrents", body: body, timeout: 30)
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

private struct SearchReleaseArtwork: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    let name: String
    let identity: String
    @State private var data: Data?
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
                if let cached = Self.cache.object(forKey: key as NSString) { data = cached as Data; return }
                let request: Task<Data?, Never>
                if let existing=Self.inFlight[key] {request=existing} else {
                    request=Task { @MainActor in
                        struct Artwork: Decodable { let data: String? }
                        defer {Self.inFlight[key]=nil}
                        do {
                            let artwork: Artwork = try await store.request("POST", "/v1/search/torrents/artwork", body: ["name": name], timeout: 20)
                            guard let encoded=artwork.data,let decoded=Data(base64Encoded:encoded) else {return nil}
                            Self.cache.totalCostLimit=8*1024*1024
                            Self.cache.setObject(decoded as NSData,forKey:key as NSString,cost:decoded.count)
                            return decoded
                        } catch {return nil}
                    }
                    Self.inFlight[key]=request
                }
                let loaded=await request.value
                if !Task.isCancelled {data=loaded}
            }
    }
}

@MainActor
private enum SearchReleaseSizes {
    struct Choice: Decodable, Identifiable { let id: String; let name: String; let url: String? }
    struct Summary: Decodable { let sizeLabel: String?; let choices: [Choice] }
    static var summaries: [String: Summary] = [:]
    static var cache: [String: String] = [:]
    static var active = 0
    static func load(_ item: TorrentSearchItem, store: Store) async throws -> String {
        let key = store.server + item.sourceKey
        if let cached = cache[key] { return cached }
        while active >= 2 { try await Task.sleep(for: .milliseconds(200)) }
        try Task.checkCancellation()
        if let cached = cache[key] { return cached }
        active += 1
        defer { active -= 1 }
        guard let source = item.sources.first(where: { $0.provider == "1lou" }) else { return "" }
        let summary: Summary = try await store.request("POST", "/v1/search/torrents/summary", body: ["provider": source.provider, "id": source.id], timeout: 25)
        let value = summary.sizeLabel ?? ""
        if cache.count >= 128 { cache.removeAll(); summaries.removeAll() }
        summaries[key] = summary
        cache[key] = value
        return value
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
    @State private var fetchedSize: String?
    @State private var showBTDetails = false
    @State private var btChoices: [SearchReleaseSizes.Choice] = []
    @State private var adding = false
    @State private var added = false
    @State private var addError: String?
    private var isBT: Bool { item.sources.contains { $0.provider == "1lou" } }
    private var displayedSize: String { item.size.isEmpty ? (fetchedSize ?? "") : item.size }
    private var metadata: String { [item.edition, item.quality].filter { !$0.isEmpty }.joined(separator: " · ") }
    private var tracks: String { (item.hints.audio + [item.languages]).filter { !$0.isEmpty }.joined(separator: " · ") }
    var body: some View {
        HStack(alignment: .center, spacing: 10) {
            let episodic = item.hints.season != nil || item.hints.episodeCount != nil
            // A year borrowed from same-titled results is sent as a plain "Title Year S01" name.
            SearchReleaseArtwork(name: item.hints.year == nil && artworkYear != nil ? "\(item.title) \(artworkYear!)\(episodic ? " S01" : "")" : item.name,
                identity: "\(item.title.lowercased()):\((item.hints.year ?? artworkYear).map(String.init) ?? ""):\(episodic ? "tv" : "movie")")
            VStack(alignment: .leading, spacing: 2) {
                Text(item.title).font(.title3.bold()).foregroundStyle(.primary).lineLimit(2)
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
                if let ownership {
                    Label(LocalizedStringKey(ownership), systemImage: "checkmark.circle.fill").font(.caption.weight(.semibold)).foregroundStyle(Theme.accent)
                }
                if let addError { Text(addError).font(.caption).foregroundStyle(.orange) }
            }
            if let onTrial {
                Spacer(minLength: 8)
                if isBT {
                    if btChoices.count == 1, let choice = btChoices.first, let url = choice.url {
                        Button(trialSelected.contains(url) ? "Selected" : "Add to trial") { onTrial("torrent_url",url,item.name,item.sourceKey) }
                    } else if !btChoices.isEmpty {
                        Menu("Add to trial") { ForEach(btChoices) { choice in
                            if let url=choice.url { Button(choice.name) { onTrial("torrent_url",url,choice.name,"") } }
                        } }
                    } else { ProgressView().controlSize(.small) }
                } else if !item.magnet.isEmpty {
                    Button(trialSelected.contains(item.magnet) ? "Selected" : "Add to trial") { onTrial("magnet",item.magnet,item.name,item.sourceKey) }
                }
            } else if isBT {
                Spacer(minLength: 8)
                if adding { ProgressView().controlSize(.small) }
                else if added { Image(systemName: "checkmark.circle.fill").foregroundStyle(Theme.accent).accessibilityLabel("Added") }
                else if btChoices.count == 1, let choice = btChoices.first {
                    Button("Add torrent") { Task { await addBT(choice) } }.buttonStyle(MonoButtonStyle(prominent: true))
                } else if btChoices.count > 1 {
                    Menu("Add torrent") { ForEach(btChoices) { choice in Button(choice.name) { Task { await addBT(choice) } } } }
                        .buttonStyle(MonoButtonStyle(prominent: true))
                }
                Button { showBTDetails=true } label: { Image(systemName:"info.circle") }
                    .buttonStyle(.borderless).accessibilityLabel("Torrent details")
            }
        }.padding(.vertical, 2).frame(maxWidth: .infinity, alignment: .leading).contentShape(Rectangle())
            .sheet(isPresented: $showBTDetails) {
                NavigationStack { TorrentSearchDetail(item:item, loadBTDetails:true, knownSize:displayedSize)
                    .toolbar { ToolbarItem(placement:.cancellationAction) { Button("Close") { showBTDetails=false } } }
                }
                    .frame(minWidth:320,minHeight:400)
            }
            .task(id: store.server + item.sourceKey) {
                guard isBT else { return }
                do {
                    fetchedSize = try await SearchReleaseSizes.load(item, store: store)
                    btChoices = SearchReleaseSizes.summaries[store.server + item.sourceKey]?.choices ?? []
                }
                catch is CancellationError { }
                catch { fetchedSize = nil }
            }
    }
    private func addBT(_ choice: SearchReleaseSizes.Choice) async {
        guard !adding, !added, let url=choice.url else { return }
        adding=true; addError=nil
        defer { adding=false }
        do {
            struct Result: Decodable { let torrentDuplicate: TorrentIdentity? }
            struct TorrentIdentity: Decodable { let name: String? }
            let result: Result = try await store.request("POST", "/v1/torrents", body: ["torrent_url":url,"listing_key":btChoices.count == 1 ? item.sourceKey : ""], timeout:45)
            added=true
            store.notify(result.torrentDuplicate == nil ? "Torrent added" : "Already in your torrents", detail:item.name)
            onAdded()
        } catch { addError=error.localizedDescription }
    }

}

private struct TorrentSearchDetail: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) private var dismiss
    let item: TorrentSearchItem
    var embedded = false
    var loadBTDetails = false
    var knownSize = ""
    var onAdded: () -> Void = {}
    @State private var detail: TorrentSearchDetails?
    @State private var attachment = 0
    private var release: TorrentSearchDetails.Release? {
        guard let values = detail?.releases, values.indices.contains(attachment) else { return nil }
        return values[attachment]
    }
    private var resolved: TorrentSearchItem { release?.item ?? item }
    private var displayedSize: String { resolved.size.isEmpty ? knownSize : resolved.size }
    private var isBTListing: Bool { item.sources.contains { $0.provider == "1lou" } }
    @State private var loading = false
    @State private var adding = false
    @State private var paused = false
    @State private var added = false
    @State private var message: String?
    @State private var detailError: String?
    private typealias Choice = SearchReleaseSizes.Choice
    private struct Resolution: Decodable { let choices: [Choice]?; let item: TorrentSearchItem?; let metainfo: String? }
    @State private var choices: [Choice] = []
    @State private var selectedAttachment: String?

    var body: some View {
        List {
            SwiftUI.Section {
                Text(item.title).font(.title2.bold()).searchSelectable
                if !item.edition.isEmpty { Text(item.edition).foregroundStyle(.secondary) }
                Text([item.quality, displayedSize].filter { !$0.isEmpty }.joined(separator: " · ")).font(.subheadline).foregroundStyle(.secondary)
                if item.seeders != nil { Label(item.availability, systemImage: "arrow.up.circle.fill").font(.subheadline).foregroundStyle(Theme.accent) }
                Text(item.name).font(.caption).foregroundStyle(.secondary).searchSelectable
                if let releases = detail?.releases, releases.count > 1 {
                    Picker("Release", selection: $attachment) {
                        ForEach(Array(releases.enumerated()), id: \.offset) { index, release in
                            Text(release.item.name).tag(index)
                        }
                    }
                }
                if !choices.isEmpty {
                    Picker("Release", selection: $selectedAttachment) {
                        Text("Choose a release").tag(String?.none)
                        ForEach(choices) { choice in Text(choice.name).tag(Optional(choice.id)) }
                    }
                }
                Toggle("Add paused", isOn: $paused).disabled(adding || added)
                Button { Task { await add() } } label: {
                    Label(LocalizedStringKey(added ? "Added to Transmission" : adding ? "Adding…" : "Add torrent"), systemImage: added ? "checkmark.circle.fill" : "plus.circle.fill")
                }.buttonStyle(MonoButtonStyle(prominent: true)).disabled(adding || added)
                if let message { Text(L10n.text(message)).foregroundStyle(.secondary) }
            }
            if !isBTListing && !item.hints.summary.isEmpty {
                SwiftUI.Section("Release tags") { Text(item.hints.summary) }
            }
            if !isBTListing { SwiftUI.Section("Sources") {
                ForEach(item.sources, id: \.url) { source in
                    #if os(tvOS)
                    Text(source.label)
                    #else
                    Link(source.label, destination: URL(string: source.url)!)
                    #endif
                }
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
            if let summary = SearchReleaseSizes.summaries[store.server + item.sourceKey] {
                if summary.choices.count == 1 { selectedAttachment = summary.choices.first?.id }
                else if summary.choices.count > 1 { choices = summary.choices }
            }
            guard !isBTListing || loadBTDetails else { return }
            loading = true; defer { loading = false }
            do { detail = try await store.request("POST", "/v1/search/torrents/detail", body: ["provider": source.provider, "id": source.id, "explicit": loadBTDetails], timeout: 45) }
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
            if !resolved.magnet.isEmpty && choices.isEmpty {
                payload["magnet"] = resolved.magnet
            } else if let source = item.sources.first(where: { $0.provider == "1lou" }) {
                if selectedAttachment == nil, let summary = SearchReleaseSizes.summaries[store.server + item.sourceKey] {
                    if summary.choices.count == 1 { selectedAttachment = summary.choices.first?.id }
                    else if summary.choices.count > 1 { choices = summary.choices; message = "Choose a release, then add it."; return }
                }
                var request: [String: Any] = ["provider": source.provider, "id": source.id]
                if let selectedAttachment { request["attachment"] = selectedAttachment }
                let resolution: Resolution = try await store.request("POST", "/v1/search/torrents/resolve", body: request, timeout: 45)
                if let options = resolution.choices, !options.isEmpty {
                    choices = options; message = "Choose a release, then add it."; return
                }
                guard let metainfo = resolution.metainfo else { message = "Couldn’t retrieve this torrent. Try again."; return }
                payload["metainfo"] = metainfo
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
