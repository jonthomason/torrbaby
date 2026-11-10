import SwiftUI

struct LibraryDestination: Hashable { let id: String }
struct LibraryShelfDestination: Hashable { let name: String; let ids: [String] }

struct MediaLibrary: Decodable {
    let titles: [LibraryTitle]
    let lastScan: Double?
    let scanning: Bool
    let error: String?
}
struct EpisodeNumber: Codable, Hashable {
    let season: Int
    let episode: Int
    var label: String { String(format: "S%02dE%02d", season, episode) }
    func missingMessage(in known: [EpisodeNumber]) -> String? {
        guard episode >= 0, episode < Int.max, season >= 0, season < Int.max else { return nil }
        let later = known.filter { $0.season == season && $0.episode > episode }.map(\.episode).min()
        let missingSeason: Int
        let first: Int
        let end: Int
        if let later, later > episode + 1 {
            missingSeason = season; first = episode + 1; end = later - 1
        } else if later == nil, season > 0,
                  let next = known.filter({ $0.season == season + 1 }).map(\.episode).min(), next > 1 {
            missingSeason = season + 1; first = 1; end = next - 1
        } else if later == nil, season > 0,
                  let following = known.filter({ $0.season > season }).map(\.season).min(), following > season + 1 {
            return following == season + 2 ? "Missing Season \(season + 1)" : "Missing Seasons \(season + 1)–\(following - 1)"
        } else { return nil }
        let start = EpisodeNumber(season: missingSeason, episode: first).label
        return first == end ? "Missing \(start)" : "Missing \(start)–E\(String(format: "%02d", end))"
    }
    func next(in known: [EpisodeNumber], allowSeasonChange: Bool = true) -> EpisodeNumber? {
        guard season >= 0, season < Int.max, episode >= 0, episode < Int.max else { return nil }
        let adjacent = EpisodeNumber(season: season, episode: episode + 1)
        if known.contains(adjacent) { return adjacent }
        guard allowSeasonChange, season > 0,
              !known.contains(where: { $0.season == season && $0.episode > episode }) else { return nil }
        let nextSeason = EpisodeNumber(season: season + 1, episode: 1)
        return known.contains(nextSeason) ? nextSeason : nil
    }
}
struct LibraryTitle: Decodable, Identifiable {
    let id: String
    let kind: String
    let title: String
    let year: String?
    let genres: [String]
    let summary: String
    let contentRating: String?
    let durationMinutes: Double?
    let poster: String?
    let backdrop: String?
    let addedAt: Double
    let assets: [LibraryAsset]
    var playable: [LibraryAsset] { assets.filter { $0.complete && $0.role != "extra" }.sorted { ($0.season ?? 0, $0.episode ?? 0, $0.name ?? "") < ($1.season ?? 0, $1.episode ?? 0, $1.name ?? "") } }
    var resume: LibraryAsset? { playable.filter { !$0.watched && $0.position > 0 }.max { $0.lastPlayed < $1.lastPlayed } }
    var episodeOrder: [EpisodeNumber] {
        assets.filter { $0.role != "extra" }.compactMap { asset in
            guard let season = asset.season, let episode = asset.episode else { return nil }
            return EpisodeNumber(season: season, episode: episode)
        }
    }
    func autoplayStopMessage(after assetID: String) -> String {
        guard let current = assets.first(where: { $0.id == assetID }), let season = current.season, let episode = current.episode else { return "Playback finished" }
        let number = EpisodeNumber(season: season, episode: episode)
        if let gap = number.missingMessage(in: episodeOrder) { return gap }
        if let next = number.next(in: episodeOrder) {
            return assets.contains { $0.season == next.season && $0.episode == next.episode && $0.complete && $0.role != "extra" }
                ? "Couldn’t start \(next.label)" : "\(next.label) is still downloading"
        }
        return "No more episodes available"
    }
    func nextEpisode(after assetID: String) -> LibraryAsset? {
        guard kind == "tv", let current = assets.first(where: { $0.id == assetID }),
              current.role != "extra", let season = current.season, let episode = current.episode,
              let next = EpisodeNumber(season: season, episode: episode).next(in: episodeOrder) else { return nil }
        return assets.filter { $0.role != "extra" && $0.season == next.season && $0.episode == next.episode }
            .sorted { ($0.complete ? 0 : 1, $0.id) < ($1.complete ? 0 : 1, $1.id) }.first
    }

}
struct LibraryAsset: Decodable, Identifiable {
    let id: String
    let name: String?
    let role: String?
    let season: Int?
    let episode: Int?
    let summary: String
    let thumbnail: String?
    let complete: Bool
    let position: Double
    let duration: Double
    let watched: Bool
    let lastPlayed: Double
    var mediaInfo: MediaTrackInfo? = nil
}

struct MediaTrackInfo: Decodable {
    struct Track: Decodable { let language: String?; let codec: String?; let extractionSupported: Bool? }
    struct Prepared: Decodable { let id: String; let language: String?; let generated: Bool; let state: String }
    let state: String
    let audioTracks: [Track]
    let subtitleTracks: [Track]
    let prepared: [Prepared]
}
struct MediaLanguagesView: View {
    let info: MediaTrackInfo?
    private func name(_ code: String?) -> String {
        guard let raw = code?.trimmingCharacters(in: .whitespacesAndNewlines), !raw.isEmpty else { return "" }
        let code = raw.lowercased().replacingOccurrences(of: "_", with: "-")
        guard !["und", "unknown", "null", "mul", "zxx"].contains(code) else { return "" }
        let normalized = ["eng":"en", "zho":"zh", "chi":"zh", "jpn":"ja", "spa":"es", "fra":"fr", "fre":"fr", "deu":"de" ][code] ?? code
        return Locale.current.localizedString(forIdentifier: normalized) ?? code
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let info, info.state == "ready" {
                languageRow("Audio", icon: "speaker.wave.2", languages: info.audioTracks.map { name($0.language) }.filter { !$0.isEmpty })
                VStack(alignment: .leading, spacing: 3) {
                    Label("Subtitles", systemImage: "captions.bubble").font(.caption.weight(.semibold))
                    let original = Array(Set(
                        info.subtitleTracks.filter { $0.extractionSupported != false }.map { name($0.language) }
                        + info.prepared.filter { !$0.generated && $0.state == "ready" }.map { name($0.language) }
                    )).filter { !$0.isEmpty }.sorted()
                    let generated = info.prepared.filter { $0.generated && $0.state == "ready" && !name($0.language).isEmpty }
                    HStack(spacing: 6) {
                        if !original.isEmpty { Text(original.joined(separator: " · ")) }
                        ForEach(Array(generated.enumerated()), id: \.element.id) { index, track in
                            if !original.isEmpty || index > 0 { Text("·") }
                            Text(name(track.language))
                            Image("TorrbabyLogo").resizable().frame(width: 18, height: 18)
                                .clipShape(RoundedRectangle(cornerRadius: 3))
                                .accessibilityLabel("Generated subtitles")
                        }
                        if original.isEmpty && generated.isEmpty { Text("None") }
                    }
                }
            } else {
                Label(info?.state == "failed" ? "Languages unavailable" : "Checking languages…", systemImage: "captions.bubble")
            }
        }.font(.callout).foregroundStyle(.secondary)
    }
    private func languageRow(_ title: String, icon: String, languages: [String]) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            Label(title, systemImage: icon).font(.caption.weight(.semibold))
            Text(languages.isEmpty ? "Available" : Array(Set(languages)).sorted().joined(separator: " · "))
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

struct LibraryArtwork: View {
    @EnvironmentObject var store: Store
    let id: String
    var variant = "poster"
    @State private var data: Data?
    private static let cache = NSCache<NSString, NSData>()
    var body: some View {
        GeometryReader { geometry in
          ZStack {
            Theme.panel
            if let data {
                #if os(macOS)
                if let image = NSImage(data: data) { Image(nsImage: image).resizable().scaledToFill() }
                #else
                if let image = UIImage(data: data) { Image(uiImage: image).resizable().scaledToFill() }
                #endif
            } else {
                Image(systemName: "film").font(.largeTitle).foregroundStyle(Theme.muted)
            }
          }.frame(width: geometry.size.width, height: geometry.size.height).clipped()
        }
        .task(id: store.server + id + variant) {
            data = nil
            let key = (store.server + id + variant) as NSString
            if let cached = Self.cache.object(forKey: key) { data = cached as Data; return }
            struct Artwork: Decodable { let data: String? }
            do {
                let image: Artwork = try await store.request("GET", "/v1/library/artwork/\(id)/\(variant)")
                if let encoded = image.data, let decoded = Data(base64Encoded: encoded) {
                    data = decoded
                    Self.cache.totalCostLimit = 48 * 1024 * 1024
                    Self.cache.setObject(decoded as NSData, forKey: key, cost: decoded.count)
                }
            } catch { /* Missing artwork should not block browsing. */ }
        }
    }
}

struct LibraryBrowser: View {
    @EnvironmentObject var store: Store
    var kind: String? = nil
    var shelfIDs: [String]? = nil
    var showDownloads: () -> Void = {}
    @State private var search = ""
    #if os(macOS)
    @FocusState private var searchFocused: Bool
    #endif
    @State private var genre = "All genres"
    @State private var order = "Recently added"
    var titles: [LibraryTitle] {
        let result = store.library?.titles.filter { title in
            (shelfIDs == nil || shelfIDs!.contains(title.id)) && (kind == nil || title.kind == kind) && (genre == "All genres" || title.genres.contains(genre)) &&
            (search.isEmpty || title.title.localizedCaseInsensitiveContains(search) || title.genres.contains { $0.localizedCaseInsensitiveContains(search) })
        } ?? []
        return result.sorted {
            if order == "Release date" { return ($0.year ?? "", $0.title) > ($1.year ?? "", $1.title) }
            if order == "Title" { return $0.title.localizedStandardCompare($1.title) == .orderedAscending }
            return $0.addedAt > $1.addedAt
        }
    }
    var genres: [String] { Array(Set((store.library?.titles ?? []).filter { kind == nil || $0.kind == kind }.flatMap(\.genres))).sorted() }
    var browsing: Bool { shelfIDs == nil && kind == nil && search.isEmpty && genre == "All genres" }
    private var homeShelves: [(name: String, items: [LibraryTitle], allItems: [LibraryTitle])] {
        var result: [(name: String, items: [LibraryTitle], allItems: [LibraryTitle])] = []
        var shown = Set<String>()
        func append(_ name: String, _ candidates: [LibraryTitle]) {
            let fresh = candidates.filter { !shown.contains($0.id) }
            guard !fresh.isEmpty else { return }
            result.append((name, fresh, candidates))
            shown.formUnion(fresh.prefix(8).map(\.id))
        }
        append("Continue watching", titles.filter { $0.resume != nil }.sorted { $0.continueWatchingAt > $1.continueWatchingAt })
        append("Recently added", titles)
        append("Movies", titles.filter { $0.kind == "movie" })
        append("TV Shows", titles.filter { $0.kind == "tv" })
        for genre in genres { append(genre, titles.filter { $0.genres.contains(genre) }) }
        return result
    }
    private var remainingTitles: [LibraryTitle] {
        let shown = Set(homeShelves.flatMap { $0.items.prefix(8).map(\.id) })
        return titles.filter { !shown.contains($0.id) }
    }
    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 28) {
                if browsing, let featured = titles.homeHero {
                    NavigationLink(value: LibraryDestination(id: featured.id)) {
                        LibraryArtwork(id: featured.id, variant: "backdrop")
                            .frame(height: 360)
                            .overlay(LinearGradient(colors: [.clear, .black.opacity(0.92)], startPoint: .top, endPoint: .bottom))
                            .overlay(alignment: .bottomLeading) {
                            VStack(alignment: .leading, spacing: 10) {
                                Text(featured.title).font(.system(size: 38, weight: .bold, design: .serif))
                                    .lineLimit(3).minimumScaleFactor(0.7).multilineTextAlignment(.leading)
                                Text(([featured.year ?? ""] + featured.genres.prefix(2)).filter { !$0.isEmpty }.joined(separator: " · ")).font(.subheadline)
                                Label("Explore", systemImage: "play.circle.fill").font(.headline).padding(.top, 5)
                            }.padding(28).foregroundStyle(.white)
                        }.clipShape(RoundedRectangle(cornerRadius: 24))
                    }.buttonStyle(.plain)
                }
                if !titles.isEmpty { HStack {
                    Picker("Genre", selection: $genre) {
                        Text("All genres").tag("All genres")
                        ForEach(genres, id: \.self) { Text($0).tag($0) }
                    }
                    Picker("Sort", selection: $order) {
                        ForEach(["Recently added", "Release date", "Title"], id: \.self) { Text($0).tag($0) }
                    }
                }.frame(maxWidth: 600) }
                if !store.offline, let error = store.libraryError ?? store.library?.error { Text(error).font(.callout).foregroundStyle(.orange) }
                if titles.isEmpty {
                    if store.offline {
                        ContentUnavailableView {
                            Label("Offline mode", systemImage: "wifi.slash")
                        } description: {
                            Text("Watch your downloaded movies and episodes.")
                        } actions: {
                            Button(action: showDownloads) { Label("Downloads", systemImage: "arrow.down.circle.fill") }.buttonStyle(.borderedProminent)
                        }
                    } else if store.configured && store.library == nil && store.libraryError == nil {
                        ProgressView("Loading library…").frame(maxWidth: .infinity).padding(.vertical, 60)
                    } else if store.library == nil {
                        ContentUnavailableView("Library unavailable", systemImage: "film.stack")
                    } else {
                        ContentUnavailableView("Your library", systemImage: "film.stack", description: Text(search.isEmpty ? "No movies or shows yet." : "No titles match your search."))
                    }
                } else if browsing && order == "Recently added" {
                    ForEach(homeShelves, id: \.name) { section in shelf(section.name, section.items, allItems: section.allItems) }
                    if !remainingTitles.isEmpty {
                        Text("More to explore").font(.title2.bold())
                        LazyVGrid(columns: [GridItem(.adaptive(minimum: 155, maximum: 240), spacing: 20)], alignment: .leading, spacing: 28) {
                            ForEach(remainingTitles) { title in poster(title) }
                        }
                    }
                } else {
                    LazyVGrid(columns: [GridItem(.adaptive(minimum: 155, maximum: 240), spacing: 20)], alignment: .leading, spacing: 28) {
                        ForEach(titles) { title in poster(title) }
                    }
                }
            }.padding(24)
        }.background(Theme.canvas)
        .searchable(text: $search, prompt: "Search movies and TV shows")
        #if os(macOS)
        .searchFocused($searchFocused)
        .focusedSceneValue(\.searchContent, { searchFocused = true })
        #endif
    }
    @ViewBuilder func shelf(_ name: String, _ items: [LibraryTitle], allItems: [LibraryTitle]? = nil) -> some View {
        if !items.isEmpty {
            VStack(alignment: .leading, spacing: 14) {
                HStack {
                    Text(name).font(.title2.bold())
                    Spacer()
                    NavigationLink(value: LibraryShelfDestination(name: name, ids: (allItems ?? items).map(\.id))) { Text("See All").font(.subheadline.weight(.semibold)) }
                }
                ScrollView(.horizontal) {
                    LazyHStack(alignment: .top, spacing: 18) {
                        ForEach(Array(items.prefix(8))) { title in poster(title).frame(width: 175) }
                    }.padding(.vertical, 6)
                }.scrollIndicators(.hidden)
            }
        }
    }
    func poster(_ title: LibraryTitle) -> some View {
        NavigationLink(value: LibraryDestination(id: title.id)) {
            VStack(alignment: .leading, spacing: 8) {
                LibraryArtwork(id: title.id).aspectRatio(2/3, contentMode: .fit).clipShape(RoundedRectangle(cornerRadius: 12))
                Text(title.title).font(.headline).lineLimit(2).multilineTextAlignment(.leading)
                Text((title.year ?? "") + (title.kind == "tv" ? " · TV Show" : "")).font(.caption).foregroundStyle(Theme.muted)
                if title.playable.isEmpty { Label("Downloading", systemImage: "arrow.down").font(.caption2).foregroundStyle(Theme.mint) }
            }.foregroundStyle(.primary)
        }.buttonStyle(.plain)
    }
}

struct LibraryDetail: View {
    @EnvironmentObject var store: Store
    #if os(macOS)
    @Environment(\.openWindow) private var openWindow
    #endif
    let titleID: String
    @State private var season: Int = -1
    @State private var playback: Playback?
    @State private var loading = false
    @State private var preparation = ""
    @State private var playbackTask: Task<Void, Never>?
    var title: LibraryTitle? { store.library?.titles.first { $0.id == titleID } }
    var body: some View {
        ScrollView {
            if let title {
                VStack(alignment: .leading, spacing: 24) {
                    LibraryArtwork(id: title.id, variant: title.backdrop != nil ? "backdrop" : "poster")
                        .frame(height: 330).clipShape(RoundedRectangle(cornerRadius: 22))
                    Text(title.title).font(.largeTitle.bold())
                    Text(([title.year ?? "", title.contentRating ?? ""] + title.genres).filter { !$0.isEmpty }.joined(separator: " · ")).foregroundStyle(Theme.muted)
                    if let asset = title.resume ?? title.playable.first(where: { !$0.watched }) ?? title.playable.first {
                        HStack(alignment: .center, spacing: 16) {
                            Button { play(asset, title: title.title) } label: {
                                Label(asset.position > 0 && !asset.watched ? "Resume" : "Play", systemImage: "play.fill").font(.headline).padding(8)
                            }.buttonStyle(.borderedProminent).disabled(loading)
                            #if !os(tvOS)
                            DownloadAssetControl(asset: asset, title: title)
                            #endif
                        }
                    }
                    NavigationLink {
                        TorrentSearchView(initialQuery: title.title, year: title.kind == "movie" ? Int(title.year ?? "") : nil, season: title.kind == "tv" && season >= 0 ? season : nil)
                    } label: { Label("Find releases", systemImage: "magnifyingglass").font(.subheadline.weight(.semibold)) }
                    if loading {
                        HStack {
                            ProgressView().controlSize(.small)
                            Text(preparation.isEmpty ? "Preparing playback…" : preparation).font(.caption)
                            Button("Cancel") { playbackTask?.cancel() }
                        }
                    }
                    if let asset = title.resume ?? title.playable.first(where: { !$0.watched }) ?? title.playable.first {
                        MediaLanguagesView(info: asset.mediaInfo)
                    }
                    Text(title.summary).font(.body).lineSpacing(5).frame(maxWidth: 900, alignment: .leading)
                    if title.kind == "tv" {
                        let seasons = Array(Set(title.assets.compactMap(\.season))).sorted()
                        Picker("Season", selection: $season) {
                            Text("All seasons").tag(-1)
                            ForEach(seasons, id: \.self) { Text($0 == 0 ? "Specials" : "Season \($0)").tag($0) }
                        }.frame(maxWidth: 340)
                    }
                    ForEach(title.assets.filter { $0.role != "extra" && (season == -1 || $0.season == season) }.sorted { ($0.season ?? 0, $0.episode ?? 0) < ($1.season ?? 0, $1.episode ?? 0) }) { asset in
                        Button { play(asset, title: title.title) } label: {
                            HStack(spacing: 16) {
                                Image(systemName: asset.watched ? "checkmark.circle.fill" : "play.circle.fill").font(.title).foregroundStyle(Theme.mint)
                                VStack(alignment: .leading, spacing: 5) {
                                    if let s = asset.season, let e = asset.episode { Text("S\(s) · E\(e)").font(.caption).foregroundStyle(Theme.muted) }
                                    Text(asset.name ?? title.title).font(.headline)
                                    if !asset.summary.isEmpty { Text(asset.summary).font(.caption).lineLimit(3).foregroundStyle(Theme.muted) }
                                    if asset.duration > 0 && !asset.watched { ProgressView(value: min(1, asset.position / asset.duration)) }
                                    if !asset.complete { Text("Downloading").font(.caption).foregroundStyle(Theme.muted) }
                                }
                                Spacer()
                            }.padding(18).background(Theme.panel, in: RoundedRectangle(cornerRadius: 14))
                        }.buttonStyle(.plain).disabled(!asset.complete || loading)
                    }
                    let extras = title.assets.filter { $0.role == "extra" }.sorted { ($0.name ?? "").localizedStandardCompare($1.name ?? "") == .orderedAscending }
                    if !extras.isEmpty {
                        Text("Special features").font(.title2.bold())
                        ForEach(extras) { asset in
                            Button { play(asset, title: asset.name ?? title.title) } label: {
                                HStack(spacing: 16) {
                                    Image(systemName: asset.watched ? "checkmark.circle.fill" : "play.circle.fill").font(.title).foregroundStyle(Theme.mint)
                                    VStack(alignment: .leading, spacing: 5) {
                                        Text(asset.name ?? "Special feature").font(.headline)
                                        if asset.duration > 0 && !asset.watched { ProgressView(value: min(1, asset.position / asset.duration)) }
                                        if !asset.complete { Text("Downloading").font(.caption).foregroundStyle(Theme.muted) }
                                    }
                                    Spacer()
                                }.padding(18).background(Theme.panel, in: RoundedRectangle(cornerRadius: 14))
                            }.buttonStyle(.plain).disabled(!asset.complete || loading)
                        }
                    }
                }.padding(24)
            }
        }.background(Theme.canvas).navigationTitle(title?.title ?? "Library")
        .onDisappear { playbackTask?.cancel() }
        #if os(iOS)
        .background(MobilePlayerPresentation(playback: $playback, store: store).frame(width: 0, height: 0))
        #elseif os(tvOS)
        .fullScreenCover(item: $playback) { PlayerScreen(playback: $0, store: store).onExitCommand { playback = nil } }
        #endif
    }
    func play(_ asset: LibraryAsset, title: String) {
        guard !loading else { return }
        loading = true; preparation = ""
        playbackTask = Task {
            defer { loading = false; playbackTask = nil }
            do {
                #if !os(tvOS)
                let downloads = DownloadQueue.shared
                if let id = downloads.readyDownload(assetID: asset.id, server: store.server),
                   let local = await downloads.preparePlayback(id) {
                    #if os(macOS)
                    openWindow(value: local)
                    #else
                    playback = local
                    #endif
                    return
                }
                #endif
                await store.flushProgress()
                try await store.registerClient()
                let link = try await store.mediaPlaybackLink(asset: asset, titleID: titleID) { preparation = $0 }
                let item = Playback(sessionId: link.sessionId, resumePosition: link.resumePosition ?? 0, url: try store.endpoint(link.path), name: asset.season == nil ? title : title + " · " + (asset.name ?? ""), libraryAssetID: asset.id, libraryTitleID: titleID)
                #if os(macOS)
                openWindow(value: item)
                #else
                playback = item
                #endif
            } catch { if !Task.isCancelled { store.actionError = error.localizedDescription } }
        }
    }
}

extension Array where Element == LibraryTitle {
    var homeHero: LibraryTitle? {
        let watched = filter { !$0.playable.isEmpty && $0.assets.contains { $0.lastPlayed > 0 } }
        return watched.max { ($0.assets.map(\.lastPlayed).max() ?? 0) < ($1.assets.map(\.lastPlayed).max() ?? 0) }
            ?? first(where: { $0.backdrop != nil && !$0.playable.isEmpty })
    }
}
