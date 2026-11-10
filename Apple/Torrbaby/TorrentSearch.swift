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
    let infoHash: String
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
    var id: String { infoHash.isEmpty ? sources.map { "\($0.provider):\($0.id)" }.joined(separator: ",") : infoHash }
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
    var edition: String {
        var values = [hints.year.map(String.init)]
        if let count = hints.episodeCount { values.append("Complete · \(count) episodes") }
        else if let episode = hints.episode {
            let range = hints.episodeEnd.map { "\(episode)–\($0)" } ?? "\(episode)"
            values.append("Episodes \(range)")
        }
        if let season = hints.season { values.append("Season \(season)") }
        return values.compactMap { $0 }.joined(separator: " · ")
    }
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
    let items: [TorrentSearchItem]
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
    @State private var source = "all"
    @State private var response: TorrentSearchResponse?
    @State private var error: String?
    @State private var searching = false
    @State private var work: Task<Void, Never>?
    @State private var selectedID: String?
    @FocusState private var searchFocused: Bool
    @State private var prepareNextSearch = false
    #if !os(tvOS)
    @State private var querySelection: TextSelection?
    #endif
    private var selectedItem: TorrentSearchItem? { response?.items.first { $0.id == selectedID } }
    var body: some View {
        GeometryReader { geometry in
            VStack(spacing: 0) {
                searchHeader
                if let response {
                    if response.items.isEmpty && !searching {
                        if !response.providers.isEmpty && response.providers.allSatisfy({ $0.error != nil }) {
                            let limited = response.providers.contains { $0.error?.contains("rate limiting") == true }
                            ContentUnavailableView(limited ? "Search rate limited" : "Sources unavailable", systemImage: limited ? "clock" : "network.slash", description: Text(LocalizedStringKey(response.providers.compactMap(\.error).first ?? "Try again shortly.")))
                        } else {
                            ContentUnavailableView("No releases found", systemImage: "magnifyingglass", description: Text("Try a shorter title or another source."))
                        }
                    } else if usesColumns(geometry.size.width) {
                        HStack(spacing: 0) {
                            results(response.items, inline: true)
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
                        results(response.items, inline: false)
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
    }
    private func usesColumns(_ width: CGFloat) -> Bool {
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
            }.pickerStyle(.segmented).labelsHidden().frame(maxWidth: 320)
                .onChange(of: market) {
                    work?.cancel(); searching = false; response = nil; selectedID = nil; error = nil; source = "all"
                    if query.trimmingCharacters(in: .whitespacesAndNewlines).count >= 2 { search() }
                }
            HStack(spacing: 12) {
                Image(systemName: "magnifyingglass").foregroundStyle(Theme.mint)
                searchField
                    .focused($searchFocused)
                    .onSubmit(search).autocorrectionDisabled().textFieldStyle(.plain)
                Button(action: search) { Text(LocalizedStringKey(searching ? "Searching…" : "Search")) }
                    .buttonStyle(.borderedProminent)
                    .disabled(query.trimmingCharacters(in: .whitespacesAndNewlines).count < 2 || searching)
            }.padding(12).background(Theme.panel, in: RoundedRectangle(cornerRadius: 14))
            HStack {
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
                Spacer()
                if let response { Text("\(response.items.count) releases").font(.caption).foregroundStyle(.secondary) }
                if searching { ProgressView().controlSize(.small) }
            }
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
    private func results(_ items: [TorrentSearchItem], inline: Bool) -> some View {
        List(items) { item in
            if inline {
                Button { selectedID = item.id } label: { TorrentSearchRow(item: item) }
                    .buttonStyle(.plain)
                    .listRowBackground(selectedID == item.id ? Theme.mint.opacity(0.13) : Color.clear)
                    .accessibilityAddTraits(selectedID == item.id ? .isSelected : [])
            } else {
                NavigationLink { TorrentSearchDetail(item: item, onAdded: { prepareNextSearch = true }).id(item.id) } label: { TorrentSearchRow(item: item) }
            }
        }.listStyle(.plain).disabled(searching)
    }
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
                if !value.items.contains(where: { $0.id == selectedID }) { selectedID = value.items.first?.id }
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
    @State private var data: Data?
    private static let cache = NSCache<NSString, NSData>()
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
            .task(id: store.server + name) {
                data = nil
                let key = (store.server + name) as NSString
                if let cached = Self.cache.object(forKey: key) { data = cached as Data; return }
                struct Artwork: Decodable { let data: String? }
                do {
                    let artwork: Artwork = try await store.request("POST", "/v1/search/torrents/artwork", body: ["name": name], timeout: 20)
                    guard !Task.isCancelled, let encoded = artwork.data, let decoded = Data(base64Encoded: encoded) else { return }
                    data = decoded
                    Self.cache.totalCostLimit = 8 * 1024 * 1024
                    Self.cache.setObject(decoded as NSData, forKey: key, cost: decoded.count)
                } catch { /* Optional artwork never blocks a release. */ }
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
        let key = store.server + item.id
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
    @State private var fetchedSize: String?
    private var displayedSize: String { item.size.isEmpty ? (fetchedSize ?? "") : item.size }
    private var metadata: String { [item.edition, item.quality].filter { !$0.isEmpty }.joined(separator: " · ") }
    private var tracks: String { (item.hints.audio + [item.languages]).filter { !$0.isEmpty }.joined(separator: " · ") }
    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            SearchReleaseArtwork(name: item.name)
            VStack(alignment: .leading, spacing: 2) {
                Text(item.title).font(.title3.bold()).foregroundStyle(.primary).lineLimit(2)
                if !displayedSize.isEmpty || item.seeders != nil {
                    HStack(alignment: .firstTextBaseline, spacing: 12) {
                        if !displayedSize.isEmpty {
                            Text(verbatim: displayedSize).foregroundStyle(Theme.mint)
                        }
                        if item.seeders != nil {
                            Text(item.availability).foregroundStyle(item.seeders == 0 ? Color.orange : Theme.mint)
                        }
                    }
                    .font(.headline).monospacedDigit()
                    .fixedSize(horizontal: false, vertical: true)
                }
                if !metadata.isEmpty { Text(metadata).font(.caption).foregroundStyle(.secondary).lineLimit(2) }
                if !tracks.isEmpty { Text(tracks).font(.caption).foregroundStyle(.secondary).lineLimit(2) }
            }
        }.padding(.vertical, 2).frame(maxWidth: .infinity, alignment: .leading).contentShape(Rectangle())
            .task(id: store.server + item.id) {
                guard item.size.isEmpty else { return }
                do { fetchedSize = try await SearchReleaseSizes.load(item, store: store) }
                catch is CancellationError { }
                catch { fetchedSize = nil }
            }
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
                Text([item.quality, resolved.size].filter { !$0.isEmpty }.joined(separator: " · ")).font(.subheadline).foregroundStyle(.secondary)
                if item.seeders != nil { Label(item.availability, systemImage: "arrow.up.circle.fill").font(.subheadline).foregroundStyle(Theme.mint) }
                Text(item.name).font(.caption).foregroundStyle(.secondary).searchSelectable
                if let releases = detail?.releases, releases.count > 1 {
                    Picker("Release", selection: $attachment) {
                        ForEach(Array(releases.enumerated()), id: \.offset) { index, release in
                            Text(release.item.name).tag(index)
                        }
                    }
                }
                if item.magnet.isEmpty, let release { Text(release.item.size).font(.subheadline).foregroundStyle(.secondary) }
                if !choices.isEmpty {
                    Picker("Release", selection: $selectedAttachment) {
                        Text("Choose a release").tag(String?.none)
                        ForEach(choices) { choice in Text(choice.name).tag(Optional(choice.id)) }
                    }
                }
                Toggle("Add paused", isOn: $paused).disabled(adding || added)
                Button { Task { await add() } } label: {
                    Label(LocalizedStringKey(added ? "Added to Transmission" : adding ? "Adding…" : "Add torrent"), systemImage: added ? "checkmark.circle.fill" : "plus.circle.fill")
                }.buttonStyle(.borderedProminent).disabled(adding || added)
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
            if let summary = SearchReleaseSizes.summaries[store.server + item.id] {
                if summary.choices.count == 1 { selectedAttachment = summary.choices.first?.id }
                else if summary.choices.count > 1 { choices = summary.choices }
            }
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
            if !resolved.magnet.isEmpty && choices.isEmpty {
                payload["magnet"] = resolved.magnet
            } else if let source = item.sources.first(where: { $0.provider == "1lou" }) {
                if selectedAttachment == nil, let summary = SearchReleaseSizes.summaries[store.server + item.id] {
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
