import SwiftUI

struct LibraryDestination: Hashable { let id: String }

struct MediaLibrary: Decodable {
    let titles: [LibraryTitle]
    let lastScan: Double?
    let scanning: Bool
    let error: String?
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
    @State private var search = ""
    #if os(macOS)
    @FocusState private var searchFocused: Bool
    #endif
    @State private var genre = "All genres"
    @State private var order = "Recently added"
    var titles: [LibraryTitle] {
        let result = store.library?.titles.filter { title in
            (kind == nil || title.kind == kind) && (genre == "All genres" || title.genres.contains(genre)) &&
            (search.isEmpty || title.title.localizedCaseInsensitiveContains(search) || title.genres.contains { $0.localizedCaseInsensitiveContains(search) })
        } ?? []
        return result.sorted {
            if order == "Release date" { return ($0.year ?? "", $0.title) > ($1.year ?? "", $1.title) }
            if order == "Title" { return $0.title.localizedStandardCompare($1.title) == .orderedAscending }
            return $0.addedAt > $1.addedAt
        }
    }
    var genres: [String] { Array(Set((store.library?.titles ?? []).filter { kind == nil || $0.kind == kind }.flatMap(\.genres))).sorted() }
    var browsing: Bool { kind == nil && search.isEmpty && genre == "All genres" }
    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 28) {
                if browsing, let featured = titles.first(where: { $0.backdrop != nil && !$0.playable.isEmpty }) {
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
                HStack {
                    Picker("Genre", selection: $genre) {
                        Text("All genres").tag("All genres")
                        ForEach(genres, id: \.self) { Text($0).tag($0) }
                    }
                    Picker("Sort", selection: $order) {
                        ForEach(["Recently added", "Release date", "Title"], id: \.self) { Text($0).tag($0) }
                    }
                }.frame(maxWidth: 600)
                if let error = store.libraryError ?? store.library?.error { Text(error).font(.callout).foregroundStyle(.orange) }
                if titles.isEmpty {
                    ContentUnavailableView("Your library", systemImage: "film.stack", description: Text(search.isEmpty ? "No movies or shows yet." : "No titles match your search."))
                } else if browsing && order == "Recently added" {
                    let continuing = titles.filter { $0.resume != nil }.sorted { ($0.resume?.lastPlayed ?? 0) > ($1.resume?.lastPlayed ?? 0) }
                    if !continuing.isEmpty { shelf("Continue watching", continuing) }
                    shelf("Recently added", Array(titles.prefix(16)))
                    shelf("Movies", titles.filter { $0.kind == "movie" })
                    shelf("TV Shows", titles.filter { $0.kind == "tv" })
                    ForEach(genres.prefix(6), id: \.self) { genre in shelf(genre, titles.filter { $0.genres.contains(genre) }) }
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
    @ViewBuilder func shelf(_ name: String, _ items: [LibraryTitle]) -> some View {
        if !items.isEmpty {
            VStack(alignment: .leading, spacing: 14) {
                Text(name).font(.title2.bold())
                ScrollView(.horizontal) {
                    LazyHStack(alignment: .top, spacing: 18) {
                        ForEach(items) { title in poster(title).frame(width: 175) }
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
                        Button { play(asset, title: title.title) } label: {
                            Label(asset.position > 0 && !asset.watched ? "Resume" : "Play", systemImage: "play.fill").font(.headline).padding(8)
                        }.buttonStyle(.borderedProminent).disabled(loading)
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
        #if !os(tvOS)
        .toolbar { if let title { DownloadTitleMenu(title: title) } }
        #endif
        #if os(iOS)
        .background(MobilePlayerPresentation(playback: $playback, store: store).frame(width: 0, height: 0))
        #elseif os(tvOS)
        .fullScreenCover(item: $playback) { PlayerScreen(playback: $0, store: store).onExitCommand { playback = nil } }
        #endif
    }
    func play(_ asset: LibraryAsset, title: String) {
        loading = true
        Task {
            defer { loading = false }
            do {
                await store.flushProgress()
                try await store.registerClient()
                let link: PlaybackLink = try await store.request("POST", "/v1/library/assets/\(asset.id)/play", body: ["client_id":store.clientId])
                let item = Playback(sessionId: link.sessionId, resumePosition: link.resumePosition ?? 0, url: try store.endpoint(link.path), name: asset.season == nil ? title : title + " · " + (asset.name ?? ""), libraryAssetID: asset.id, libraryTitleID: titleID)
                #if os(macOS)
                openWindow(value: item)
                #else
                playback = item
                #endif
            } catch { store.actionError = error.localizedDescription }
        }
    }
}
