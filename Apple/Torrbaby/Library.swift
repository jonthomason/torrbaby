import SwiftUI
import SQLite3
import CryptoKit

struct LibraryCastMember: Decodable {
    struct Role: Decodable { let character: String?; let episodeCount: Int? }
    let name: String
    let character: String?
    let roles: [Role]?
    let totalEpisodeCount: Int?
}

struct LibraryCast: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @Environment(\.dismiss) private var dismiss
    #if os(macOS)
    @Environment(\.openWindow) private var openWindow
    #endif
    struct Entry: Identifiable { let id: Int; let name: String; let character: String; let episodes: Int? }
    let title: String
    let television: Bool
    private let entries: [Entry]
    private let truncated: Bool
    init(title: String, members: [LibraryCastMember], television: Bool) {
        self.title = title; self.television = television
        var rows: [Entry] = []
        outer: for person in members {
            if let roles = person.roles, !roles.isEmpty {
                for role in roles {
                    rows.append(Entry(id: rows.count, name: person.name, character: role.character ?? "", episodes: role.episodeCount))
                    if rows.count > 150 { break outer }
                }
            } else {
                rows.append(Entry(id: rows.count, name: person.name, character: person.character ?? "", episodes: person.totalEpisodeCount))
                if rows.count > 150 { break }
            }
        }
        truncated = rows.count > 150; entries = Array(rows.prefix(150))
    }
    // tvOS text is about twice the size, so the count column needs twice the room.
    // A centred table: full-width columns put the name and its role too far apart to read across.
    private var tableWidth: CGFloat {
        #if os(tvOS)
        1100
        #else
        640
        #endif
    }
    private var episodesWidth: CGFloat {
        #if os(tvOS)
        170
        #else
        76
        #endif
    }
    var body: some View {
        List {
            HStack {
                Text("Actor").frame(maxWidth: .infinity, alignment: .leading)
                Text("Character").frame(maxWidth: .infinity, alignment: .leading)
                if television { Text("Episodes").lineLimit(1).fixedSize().frame(width: episodesWidth, alignment: .trailing) }
            }.font(.caption).foregroundStyle(Theme.muted).frame(maxWidth: tableWidth).frame(maxWidth: .infinity)
            ForEach(entries) { person in
                HStack(alignment: .top, spacing: 16) {
                    Text(person.name).frame(maxWidth: .infinity, alignment: .leading)
                    Text(person.character).foregroundStyle(Theme.muted).frame(maxWidth: .infinity, alignment: .leading)
                    if television { Text(LocalizedStringKey(person.episodes.map(String.init) ?? "")).monospacedDigit().frame(width: episodesWidth, alignment: .trailing) }
                }.padding(.vertical, 8).frame(maxWidth: tableWidth).frame(maxWidth: .infinity)
                #if os(tvOS)
                .focusable()
                #endif
            }
            if truncated { Text("Showing the first 150 cast entries.").foregroundStyle(Theme.muted) }
        }.canvasBackground().navigationTitle(L10n.format("Cast · %@", title))
            #if os(macOS)
            .navigationBarBackButtonHidden()
            .toolbar {
                ToolbarItemGroup(placement: .navigation) {
                    Button { dismiss() } label: { Label("Back", systemImage: "chevron.left") }
                    Button {} label: { Image(systemName: "chevron.right") }.disabled(true).help("Forward")
                    Button { MacLibraryTabs.create { openWindow(id: "library-tab") } } label: { Image(systemName: "plus") }
                        .help("New Tab (⌘T)").accessibilityLabel("New Tab")
                }
                LanguageToolbar()
            }
            #elseif os(iOS)
            .toolbar { LanguageToolbar() }
            #endif
    }
}
struct LibraryCastLink: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    let title: LibraryTitle
    var body: some View {
        if let members = title.cast, !members.isEmpty {
            NavigationLink {
                LibraryCast(title: title.title, members: members, television: title.kind == "tv")
            } label: { Label("Cast", systemImage: "person.2") }
            .libraryActionStyle(compact: true)
        }
    }
}
extension LibraryAsset {
    /// `brief` leaves out the time remaining, for buttons with little room.
    func playbackActionTitle(television: Bool, brief: Bool = false) -> String {
        let resume = position > 0 && !watched
        guard television, let season, let episode else {
            guard resume, duration > position, !brief else { return L10n.text(resume ? "Resume" : "Play") }
            let minutes = Int(ceil((duration - position) / 60))
            return minutes >= 60 ? L10n.format("Resume (%d hr %d min left)", minutes / 60, minutes % 60) : L10n.format("Resume (%d min left)", minutes)
        }
        guard resume else { return L10n.format("Start Season %d Ep %d", season, episode) }
        if duration > position && !brief { return L10n.format("Resume Season %d Ep %d (%d min remaining)", season, episode, Int(ceil((duration - position) / 60))) }
        return L10n.format("Resume Season %d Ep %d", season, episode)
    }
}

struct LibraryDestination: Hashable { let id: String }

struct MediaLibrary: Decodable {
    var titles: [LibraryTitle]
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
    /// The year a finished series last aired, and whether it is still running, when known.
    var endYear: String? = nil
    var ongoing: Bool? = nil
    /// The season selector's value for a show's extras, which follow its last season.
    static let extrasTab = Int.max
    /// A movie's year, or a series' run: "2018–2025", or "2018–" while it continues.
    var yearLabel: String {
        guard let year, !year.isEmpty else { return "" }
        guard kind == "tv" else { return year }
        if ongoing == true { return year + "–" }
        if ongoing == false, let endYear, !endYear.isEmpty, endYear != year { return year + "–" + endYear }
        return year
    }
    let genres: [String]
    let summary: String
    let contentRating: String?
    let durationMinutes: Double?
    let poster: String?
    let backdrop: String?
    let addedAt: Double
    let assets: [LibraryAsset]
    var continueWatchingHidden: Bool?
    var isContinuing: Bool { continueWatchingHidden != true && resume != nil }
    let cast: [LibraryCastMember]?
    var playable: [LibraryAsset] { assets.filter { $0.complete && $0.role != "extra" }.sorted { ($0.season ?? 0, $0.episode ?? 0, $0.name ?? "") < ($1.season ?? 0, $1.episode ?? 0, $1.name ?? "") } }
    var watched: Bool { !playable.isEmpty && playable.allSatisfy(\.watched) }
    var continueWatchingAt: Double { playable.map(\.lastPlayed).max() ?? 0 }
    var resume: LibraryAsset? {
        let inProgress = playable.filter { !$0.watched && $0.position > 0 }.max { $0.lastPlayed < $1.lastPlayed }
        if let latest = playable.filter({ $0.lastPlayed > 0 }).max(by: { $0.lastPlayed < $1.lastPlayed }),
           latest.watched, latest.lastPlayed >= (inProgress?.lastPlayed ?? 0),
           let next = nextEpisode(after: latest.id), next.complete, !next.watched { return next }
        return inProgress
    }
    var continuationLabel: String? {
        guard let next = resume, next.position == 0, let season = next.season, let episode = next.episode else { return nil }
        return "Up next · " + EpisodeNumber(season: season, episode: episode).label
    }
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
    struct Track: Decodable { let language: String?; let codec: String?; let extractionSupported: Bool?; var sdh: Bool? = nil }
    struct Prepared: Decodable { let id: String; let language: String?; let generated: Bool; let state: String; var kind: String? = nil; var sdh: Bool? = nil }
    let state: String
    let audioTracks: [Track]
    let subtitleTracks: [Track]
    let prepared: [Prepared]
}
struct MediaLanguagesView: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    let info: MediaTrackInfo?
    private func name(_ code: String?) -> String {
        guard let raw = code?.trimmingCharacters(in: .whitespacesAndNewlines), !raw.isEmpty else { return "" }
        let code = raw.lowercased().replacingOccurrences(of: "_", with: "-")
        guard !["und", "unknown", "null", "mul", "zxx"].contains(code) else { return "" }
        let normalized = ["eng":"en", "zho":"zh", "chi":"zh", "jpn":"ja", "spa":"es", "fra":"fr", "fre":"fr", "deu":"de" ][code] ?? code
        if normalized.lowercased() == "zh-hans" { return L10n.text("Chinese (Simplified)") }
        if normalized.lowercased() == "zh-hant" { return L10n.text("Chinese (Traditional)") }
        return L10n.locale.localizedString(forIdentifier: normalized) ?? code
    }
    /// A track in the file: its language, plus SDH when it has sound descriptions.
    private func originalName(_ code: String?, sdh: Bool) -> String {
        let language = name(code)
        guard sdh, !language.isEmpty else { return language }
        return SubtitleTrackName.text(language: code, kind: "text", sdh: true) ?? "\(language) \(L10n.text("SDH"))"
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let info, info.state == "ready" {
                languageRow("Audio", icon: "speaker.wave.2", languages: info.audioTracks.map { name($0.language) }.filter { !$0.isEmpty })
                VStack(alignment: .leading, spacing: 3) {
                    Label("Subtitles", systemImage: "captions.bubble").font(.caption.weight(.semibold))
                    let original = Array(Set(
                        info.subtitleTracks.filter { $0.extractionSupported != false }.map { originalName($0.language, sdh: $0.sdh == true) }
                        + info.prepared.filter { !$0.generated && $0.state == "ready" }.map { originalName($0.language, sdh: $0.sdh == true) }
                    )).filter { !$0.isEmpty }.sorted()
                    let generated = info.prepared.filter { $0.generated && $0.state == "ready" && !name($0.language).isEmpty }
                    HStack(spacing: 6) {
                        if !original.isEmpty { Text(original.joined(separator: " · ")) }
                        ForEach(Array(generated.enumerated()), id: \.element.id) { index, track in
                            if !original.isEmpty || index > 0 { Text("·") }
                            Text(SubtitleTrackName.text(language: track.language, kind: track.kind, sdh: track.sdh == true) ?? name(track.language))
                            Image("TorrbabyLogo").resizable().frame(width: 18, height: 18)
                                .clipShape(RoundedRectangle(cornerRadius: 3))
                                .accessibilityLabel("Generated subtitles")
                        }
                        if original.isEmpty && generated.isEmpty { Text("None") }
                    }
                }
            } else {
                Label(LocalizedStringKey(info?.state == "failed" ? "Languages unavailable" : "Checking languages…"), systemImage: "captions.bubble")
            }
        }.font(.callout).foregroundStyle(.secondary)
    }
    private func languageRow(_ title: String, icon: String, languages: [String]) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            Label(LocalizedStringKey(title), systemImage: icon).font(.caption.weight(.semibold))
            Text(LocalizedStringKey(languages.isEmpty ? "Available" : Array(Set(languages)).sorted().joined(separator: " · ")))
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

struct LibraryArtwork: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    let id: String
    var variant = "poster"
    var alignment: Alignment = .center
    @State private var data: Data?
    private static let cache = NSCache<NSString, NSData>()
    private var artworkKey: String {
        let title = store.library?.titles.first { $0.id == id }
        let episode = variant == "episode" ? store.library?.titles.lazy.flatMap(\.assets).first(where: { $0.id == id }) : nil
        let revision = variant == "episode" ? episode?.thumbnail : variant == "backdrop" ? title?.backdrop : title?.poster
        return [store.server, store.clientId, id, variant, revision ?? ""].joined(separator: "\n")
    }
    var body: some View {
        GeometryReader { geometry in
          ZStack(alignment: alignment) {
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
          }.frame(width: geometry.size.width, height: geometry.size.height, alignment: alignment).clipped()
        }
        .task(id: artworkKey) {
            data = nil
            let identifier = artworkKey
            let key = identifier as NSString
            Self.cache.totalCostLimit = 48 * 1024 * 1024
            if let cached = Self.cache.object(forKey: key) { data = cached as Data; return }
            if let saved = await LibraryDiskCache.shared.artwork(key: identifier) {
                guard !Task.isCancelled else { return }
                data = saved
                Self.cache.setObject(saved as NSData, forKey: key, cost: saved.count)
                return
            }
            struct Artwork: Decodable { let data: String? }
            do {
                let image: Artwork = try await store.request("GET", "/v1/library/artwork/\(id)/\(variant)")
                if let encoded = image.data, let decoded = Data(base64Encoded: encoded) {
                    guard !Task.isCancelled else { return }
                    data = decoded
                    Self.cache.totalCostLimit = 48 * 1024 * 1024
                    Self.cache.setObject(decoded as NSData, forKey: key, cost: decoded.count)
                    await LibraryDiskCache.shared.saveArtwork(decoded, key: identifier)
                }
            } catch { /* Missing artwork should not block browsing. */ }
        }
    }
}

struct LibraryBrowser: View {
    #if os(iOS)
    @Environment(\.horizontalSizeClass) private var sizeClass
    #endif
    private var compactLibrary: Bool {
        #if os(iOS)
        sizeClass == .compact
        #else
        false
        #endif
    }
    private var posterColumns: [GridItem] {
        compactLibrary ? Array(repeating: GridItem(.flexible(), spacing: 10), count: 3)
            : [GridItem(.adaptive(minimum: 155, maximum: 240), spacing: 20)]
    }
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    var kind: String? = nil
    var showDownloads: () -> Void = {}
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
    /// Home is the hero and its shelves, nothing else: no genre or sort controls.
    private var isHome: Bool { kind == nil }
    /// The same rows as the TV home page; a genre earns a row once it has seven titles.
    private var homeShelves: [(name: String, items: [LibraryTitle])] {
        var result: [(name: String, items: [LibraryTitle])] = []
        func append(_ name: String, _ items: [LibraryTitle]) { if !items.isEmpty { result.append((name, items)) } }
        append("Continue watching", titles.filter { $0.isContinuing }.sorted { $0.continueWatchingAt > $1.continueWatchingAt })
        append("Recently added", titles)
        append("Movies", titles.filter { $0.kind == "movie" })
        append("TV Shows", titles.filter { $0.kind == "tv" })
        append("Recently watched", titles.filter { $0.watched }.sorted { ($0.assets.map(\.lastPlayed).max() ?? 0) > ($1.assets.map(\.lastPlayed).max() ?? 0) })
        for genre in genres {
            let matches = titles.filter { $0.genres.contains(genre) }
            if matches.count >= 7 { append(genre, matches) }
        }
        return result
    }
    private var genreMenu: some View {
        Menu {
            Picker("Genre", selection: $genre) {
                Text("All genres").tag("All genres")
                ForEach(genres, id: \.self) { Text($0).tag($0) }
            }
        } label: { Label(LocalizedStringKey(genre), systemImage: "line.3.horizontal.decrease") }
            .accessibilityLabel("Genre")
    }
    private var sortMenu: some View {
        Menu {
            Picker("Sort", selection: $order) {
                ForEach(["Recently added", "Release date", "Title"], id: \.self) { Text(LocalizedStringKey($0)).tag($0) }
            }
        } label: { Label(LocalizedStringKey(order), systemImage: "arrow.up.arrow.down") }
            .accessibilityLabel("Sort")
    }
    #if os(iOS)
    @State private var controlsTucked = false
    /// Search, genre and sort sit above the first row and start scrolled out of view, so a
    /// page opens on its artwork; pulling down reveals them, like the system search drawer.
    private var pullDownControls: some View {
        let field = HStack(spacing: 6) {
            Image(systemName: "magnifyingglass").foregroundStyle(Theme.muted)
            TextField("Search movies and TV shows", text: $search).textFieldStyle(.plain).autocorrectionDisabled()
            if !search.isEmpty {
                Button { search = "" } label: { Image(systemName: "xmark.circle.fill").foregroundStyle(Theme.muted) }
                    .buttonStyle(.plain).accessibilityLabel("Clear")
            }
        }.padding(.horizontal, 10).frame(height: 38).background(Theme.control, in: RoundedRectangle(cornerRadius: Theme.radius))
        return ViewThatFits(in: .horizontal) {
            if isHome { field } else { HStack(spacing: 10) { field.frame(minWidth: 260); genreMenu; sortMenu } }
            VStack(alignment: .leading, spacing: 8) { field; HStack(spacing: 10) { genreMenu; sortMenu; Spacer(minLength: 0) } }
        }.buttonStyle(MonoButtonStyle())
    }
    #endif
    var body: some View {
        ScrollViewReader { reader in
        ScrollView {
          // The controls are outside the lazy stack so the opening offset is exact.
          VStack(alignment: .leading, spacing: 0) {
            #if os(iOS)
            pullDownControls.padding(.horizontal, compactLibrary ? 12 : 24).padding(.top, 8).padding(.bottom, 4)
            #endif
            LazyVStack(alignment: .leading, spacing: compactLibrary ? 18 : 28) {
                if browsing, let featured = titles.homeHero {
                    NavigationLink(value: LibraryDestination(id: featured.id)) {
                        LibraryArtwork(id: featured.id, variant: "backdrop")
                            .frame(height: 360)
                            .overlay(LinearGradient(colors: [.clear, .black.opacity(0.92)], startPoint: .top, endPoint: .bottom))
                            .overlay(alignment: .bottomLeading) {
                            VStack(alignment: .leading, spacing: 10) {
                                Text(featured.title).font(.system(size: 38, weight: .bold))
                                    .lineLimit(3).minimumScaleFactor(0.7).multilineTextAlignment(.leading)
                                Text(verbatim: ([featured.yearLabel] + featured.genres.prefix(2)).filter { !$0.isEmpty }.joined(separator: " · ")).font(.subheadline)
                                Label("Explore", systemImage: "play.fill").font(.headline)
                                    .padding(.horizontal, 16).padding(.vertical, 9)
                                    .foregroundStyle(Theme.onAccent).background(Theme.accent, in: RoundedRectangle(cornerRadius: Theme.radius))
                                    .padding(.top, 5)
                            }.padding(28).foregroundStyle(.white)
                        }.clipShape(RoundedRectangle(cornerRadius: 10))
                    }.buttonStyle(.plain)
                }
                #if !os(iOS)
                if !isHome && !titles.isEmpty { HStack(spacing: 10) { genreMenu; sortMenu; Spacer(minLength: 0) }.buttonStyle(MonoButtonStyle()).fixedSize(horizontal: false, vertical: true) }
                #endif
                if !store.offline, let error = store.libraryError ?? store.library?.error { Text(L10n.text(error)).font(.callout).foregroundStyle(.orange) }
                if titles.isEmpty {
                    if store.offline {
                        ContentUnavailableView {
                            Label("Offline mode", systemImage: "wifi.slash")
                        } description: {
                            Text("Watch your downloaded movies and episodes.")
                        } actions: {
                            Button(action: showDownloads) { Label("Downloads", systemImage: "arrow.down.circle.fill") }.buttonStyle(MonoButtonStyle(prominent: true))
                        }
                    } else if store.configured && store.library == nil && store.libraryError == nil {
                        ProgressView("Loading library…").frame(maxWidth: .infinity).padding(.vertical, 60)
                    } else if store.library == nil {
                        ContentUnavailableView("Library unavailable", systemImage: "film.stack")
                    } else {
                        ContentUnavailableView("Your library", systemImage: "film.stack", description: Text(LocalizedStringKey(search.isEmpty ? "No movies or shows yet." : "No titles match your search.")))
                    }
                } else if browsing {
                    ForEach(homeShelves, id: \.name) { section in shelf(section.name, section.items) }
                } else {
                    LazyVGrid(columns: posterColumns, alignment: .leading, spacing: compactLibrary ? 16 : 28) {
                        ForEach(titles) { title in poster(title) }
                    }
                }
            }.padding(compactLibrary ? 12 : 24).id("content")
          }
        }.background(Theme.canvas)
        #if os(iOS)
        // Runs once the library has content to scroll, and not again when returning to the page.
        .task(id: titles.isEmpty) {
            guard !controlsTucked, !titles.isEmpty, search.isEmpty else { return }
            try? await Task.sleep(for: .milliseconds(60))
            reader.scrollTo("content", anchor: .top)
            controlsTucked = true
        }
        #endif
        }
        #if os(macOS)
        .toolbar {
            MacSearchToolbar(text: $search, focus: $searchFocused, prompt: "Search movies and TV shows")
        }
        .focusedSceneValue(\.searchContent, { searchFocused = true })
        #elseif os(tvOS)
        .searchable(text: $search, prompt: "Search movies and TV shows")
        #endif
    }
    @ViewBuilder func shelf(_ name: String, _ items: [LibraryTitle]) -> some View {
        if !items.isEmpty {
            VStack(alignment: .leading, spacing: compactLibrary ? 8 : 14) {
                Text(L10n.text(name)).font(compactLibrary ? .headline : .title2.bold())
                ScrollView(.horizontal) {
                    LazyHStack(alignment: .top, spacing: compactLibrary ? 10 : 18) {
                        ForEach(items) { title in
                            if compactLibrary {
                                poster(title, continuing: name == "Continue watching").containerRelativeFrame(.horizontal, count: 3, spacing: 10)
                            } else { poster(title, continuing: name == "Continue watching").frame(width: 175) }
                        }
                    }.padding(.vertical, 6)
                }.scrollIndicators(.hidden)
            }
        }
    }
    func poster(_ title: LibraryTitle, continuing: Bool = false) -> some View {
        NavigationLink(value: LibraryDestination(id: title.id)) {
            VStack(alignment: .leading, spacing: 8) {
                LibraryArtwork(id: title.id).aspectRatio(2/3, contentMode: .fit)
                    .overlay { if title.poster?.isEmpty != false { Text(title.title).font(.headline).multilineTextAlignment(.center).padding(10) } }
                    .clipShape(RoundedRectangle(cornerRadius: 8))
                    .overlay(alignment: .topTrailing) { if title.watched { WatchedBadge() } }
                if continuing, let asset = title.resume {
                    GeometryReader { geometry in
                        ZStack(alignment: .leading) {
                            Rectangle().fill(.primary.opacity(0.18))
                            Rectangle().fill(.primary.opacity(0.85)).frame(width: geometry.size.width * (asset.duration > 0 ? min(1, max(0, asset.position / asset.duration)) : 0))
                        }
                    }.frame(height: 3).accessibilityHidden(true)
                    VStack(spacing: 3) {
                        if let season = asset.season, let episode = asset.episode {
                            Text(L10n.format("Season %d Ep %d", season, episode))
                        }
                        if asset.duration > 0 {
                            let minutes = Int(ceil(max(0, asset.duration - asset.position) / 60))
                            Text(minutes >= 60 ? L10n.format("%dh %dm left", minutes / 60, minutes % 60) : L10n.format("%dm left", minutes))
                        }
                    }.font(.caption).foregroundStyle(Theme.muted).frame(maxWidth: .infinity)
                } else {
                    Text(verbatim: title.yearLabel)
                        .font(.caption).foregroundStyle(Theme.muted).lineLimit(1).frame(maxWidth: .infinity)
                }
            }.foregroundStyle(.primary)
        }.buttonStyle(.plain).accessibilityLabel(title.title)
            .contextMenu { ContinueWatchingAction(title: title) }
    }
}

struct LibraryDetail: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    #if os(iOS)
    @Environment(\.horizontalSizeClass) private var sizeClass
    #endif
    /// iPhone stacks Play and Download as full-width buttons; wider layouts keep them on one row.
    private var stacksActions: Bool {
        #if os(iOS)
        sizeClass == .compact
        #else
        false
        #endif
    }
    private var contentPadding: CGFloat {
        #if os(iOS)
        sizeClass == .compact ? 12 : 24
        #else
        24
        #endif
    }
    #if os(macOS)
    @Environment(\.openWindow) private var openWindow
    #endif
    let titleID: String
    @State private var season: Int = -1
    @State private var playback: Playback?
    @State private var loading = false
    @State private var preparation = ""
    @State private var playbackTask: Task<Void, Never>?
    private var selectedSeason: Int? {
        guard let title else { return nil }
        let seasons=Array(Set(title.assets.filter { $0.role != "extra" }.compactMap(\.season))).sorted()
        if season == LibraryTitle.extrasTab, title.assets.contains(where: { $0.role == "extra" }) { return season }
        if seasons.contains(season) { return season }
        return title.resume?.season ?? title.playable.first(where: { !$0.watched && ($0.season ?? 0) > 0 })?.season ?? seasons.first(where: { $0 > 0 }) ?? seasons.first
    }
    @ViewBuilder private func playbackActions(_ asset: LibraryAsset, title: LibraryTitle) -> some View {
        Button { play(asset, title: title.title) } label: {
            Label {
                if store.canPlay(asset) {
                    // The full label when it fits on one line; otherwise without the time remaining.
                    ViewThatFits(in: .horizontal) {
                        Text(asset.playbackActionTitle(television: title.kind == "tv")).lineLimit(1).fixedSize()
                        Text(asset.playbackActionTitle(television: title.kind == "tv", brief: true)).lineLimit(2)
                    }
                } else { Text("Not downloaded") }
            } icon: { Image(systemName: store.canPlay(asset) ? "play.fill" : "wifi.slash") }
                .font(.headline).frame(minWidth: title.kind == "tv" && !stacksActions ? 120 : nil, alignment: .leading).frame(minHeight: 44)
        }.libraryActionStyle(prominent: true).disabled(loading)
        #if !os(tvOS)
        if title.kind == "tv" {
            if let number=selectedSeason, number != LibraryTitle.extrasTab { DownloadSeasonControl(title: title, season: number) }
        } else { DownloadAssetControl(asset: asset, title: title) }
        #endif
    }
    var title: LibraryTitle? { store.library?.titles.first { $0.id == titleID } }
    var body: some View {
        ScrollView {
            if let title {
                VStack(alignment: .leading, spacing: 24) {
                    ZStack(alignment: .bottomLeading) {
                        LibraryArtwork(id: title.id, variant: title.backdrop != nil ? "backdrop" : "poster", alignment: .top)
                        LinearGradient(colors: [.clear, .black.opacity(0.94)], startPoint: .top, endPoint: .bottom)
                        VStack(alignment: .leading, spacing: 14) {
                            Spacer(minLength: 160)
                            Text(title.title).font(.largeTitle.bold()).fixedSize(horizontal: false, vertical: true)
                            Text(([title.yearLabel, title.contentRating ?? ""] + title.genres.prefix(3)).filter { !$0.isEmpty }.joined(separator: " · "))
                                .font(.subheadline).foregroundStyle(.white.opacity(0.8))
                            if let asset = store.preferredLibraryAsset(title) {
                                if stacksActions {
                                    VStack(alignment: .leading, spacing: 10) { playbackActions(asset, title: title) }
                                        .environment(\.libraryActionFullWidth, true)
                                } else {
                                    HStack(spacing: 12) { playbackActions(asset, title: title) }
                                }
                            }
                        }.foregroundStyle(.white).padding(16).frame(minHeight: 380, alignment: .bottomLeading)
                    }.clipShape(RoundedRectangle(cornerRadius: 12)).environment(\.colorScheme, .dark)
                    if let asset = store.preferredLibraryAsset(title) {
                        LibraryAssetActions(asset: asset, showsDownload: false, castTitle: title)
                    }
                    if let asset = store.preferredLibraryAsset(title) {
                        MediaLanguagesView(info: asset.mediaInfo)
                    }
                    Text(title.summary).font(.body).lineSpacing(5).frame(maxWidth: 900, alignment: .leading)
                    // Extras are their own tab after the last season, not a tail on every season.
                    let extras = title.assets.filter { $0.role == "extra" }.sorted { ($0.name ?? "").localizedStandardCompare($1.name ?? "") == .orderedAscending }
                    if title.kind == "tv" {
                        let seasons = Array(Set(title.assets.filter { $0.role != "extra" }.compactMap(\.season))).sorted()
                        let tabs = extras.isEmpty ? seasons : seasons + [LibraryTitle.extrasTab]
                        ScrollView(.horizontal) {
                            HStack(spacing: 10) {
                                ForEach(tabs, id: \.self) { number in
                                    Button(LocalizedStringKey(number == LibraryTitle.extrasTab ? "Extras" : number == 0 ? "Specials" : "Season \(number)")) { season = number }
                                        .libraryActionStyle(prominent: selectedSeason == number, compact: true)
                                        .accessibilityAddTraits(selectedSeason == number ? .isSelected : [])
                                }
                            }.padding(.vertical, 4)
                        }.scrollIndicators(.hidden)
                        if selectedSeason == LibraryTitle.extrasTab {
                            ForEach(extras) { asset in episodeRow(asset, title: title) }
                        } else {
                            ForEach(title.assets.filter { $0.role != "extra" && ($0.season == selectedSeason || $0.season == nil) }.sorted { ($0.season ?? 0, $0.episode ?? 0) < ($1.season ?? 0, $1.episode ?? 0) }) { asset in
                                episodeRow(asset, title: title)
                            }
                        }
                    } else if !extras.isEmpty {
                        Text("Special features").font(.title2.bold())
                        ForEach(extras) { asset in
                            episodeRow(asset, title: title)
                        }
                    }
                }.padding(contentPadding)
            }
        }.background(Theme.canvas).navigationTitle(title?.title ?? "Library")
        .overlay(alignment: .bottom) {
            if loading {
                HStack(spacing: 12) {
                    ProgressView().controlSize(.small)
                    Text(LocalizedStringKey(preparation.isEmpty ? "Preparing playback…" : preparation)).font(.callout)
                    Button("Cancel") { playbackTask?.cancel() }
                }.padding(16).background(.regularMaterial, in: RoundedRectangle(cornerRadius: 10))
                    .padding(16)
            }
        }
        .onDisappear { playbackTask?.cancel() }
        #if os(iOS)
        .navigationBarTitleDisplayMode(.inline)
        .background(MobilePlayerPresentation(playback: $playback, store: store).frame(width: 0, height: 0))
        #elseif os(tvOS)
        .fullScreenCover(item: $playback) { PlayerScreen(playback: $0, store: store).onExitCommand { playback = nil } }
        #endif
    }
    private func episodeRow(_ asset: LibraryAsset, title: LibraryTitle) -> some View {
        HStack(spacing: 8) {
            Button { play(asset, title: title.title) } label: {
                HStack(spacing: 12) {
                    Image(systemName: "play.circle.fill").font(.title).foregroundStyle(Theme.accent)
                    VStack(alignment: .leading, spacing: 5) {
                        if asset.season != nil, let e = asset.episode { Text("Episode \(e)").font(.caption).foregroundStyle(Theme.muted) }
                        Text(asset.name ?? title.title).font(.headline)
                        if !asset.summary.isEmpty { Text(asset.summary).font(.caption).lineLimit(2).foregroundStyle(Theme.muted) }
                        if asset.duration > 0 && !asset.watched { ProgressView(value: min(1, asset.position / asset.duration)) }
                        #if !os(tvOS)
                        EpisodeDownloadProgress(asset: asset)
                        #endif
                        if store.offlineLibrary && !store.isDownloaded(asset) { Text("Not downloaded").font(.caption).foregroundStyle(Theme.muted) }
                        else if !asset.complete { Text("Downloading").font(.caption).foregroundStyle(Theme.muted) }
                    }
                    Spacer(minLength: 0)
                }.contentShape(Rectangle())
            }.buttonStyle(.plain).disabled(loading)
            LibraryEpisodeActions(asset: asset, title: title)
        }.padding(14).background(Theme.panel, in: RoundedRectangle(cornerRadius: 10))
    }
    func play(_ asset: LibraryAsset, title: String) {
        guard !loading, store.canPlay(asset) else { return }
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
                try store.requireStreamingConnection()
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

struct WatchedBadge: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    var body: some View {
        Image(systemName: "checkmark").font(.caption.bold())
            .foregroundStyle(.white).padding(7)
            .background(.black.opacity(0.65), in: Circle()).padding(8)
            .accessibilityLabel("Watched")
    }
}

private struct LibraryActionFullWidthKey: EnvironmentKey { static let defaultValue = false }
extension EnvironmentValues {
    /// On iPhone the main Play and Download actions each fill the row.
    var libraryActionFullWidth: Bool {
        get { self[LibraryActionFullWidthKey.self] }
        set { self[LibraryActionFullWidthKey.self] = newValue }
    }
}

// Shared geometry for every library detail action on Mac, iPhone and iPad.
private struct LibraryActionButtonStyle: ButtonStyle {
    var prominent = false
    /// Secondary actions and season tabs are quieter than the main Play/Download row.
    var compact = false
    @Environment(\.isEnabled) private var enabled
    @Environment(\.libraryActionFullWidth) private var fullWidth
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(compact ? .subheadline.weight(.semibold) : .headline)
            .lineLimit(fullWidth && !compact ? 1 : nil).minimumScaleFactor(fullWidth && !compact ? 0.8 : 1)
            .padding(.horizontal, compact ? 14 : 20)
            .frame(maxWidth: fullWidth && !compact ? .infinity : nil)
            .frame(minHeight: compact ? 36 : 48)
            .foregroundStyle(prominent ? Theme.onAccent : Color.white)
            .background(prominent ? Theme.accent : Theme.control, in: RoundedRectangle(cornerRadius: Theme.radius))
            .opacity(enabled ? (configuration.isPressed ? 0.7 : 1) : 0.45)
            .contentShape(RoundedRectangle(cornerRadius: Theme.radius))
    }
}

extension View {
    @ViewBuilder func libraryActionStyle(prominent: Bool = false, compact: Bool = false) -> some View {
        #if os(tvOS)
        self.buttonStyle(.bordered)
        #else
        self.buttonStyle(LibraryActionButtonStyle(prominent: prominent, compact: compact))
        #endif
    }
}

struct LibraryAssetActions: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    let asset: LibraryAsset
    var showsDownload = true
    var castTitle: LibraryTitle?
    @State private var saving = false
    var body: some View {
        LibraryActionFlow(spacing: 10) { actions }
            .font(.subheadline).libraryActionStyle(compact: true)
    }
    @ViewBuilder private var actions: some View {
        Group {
            if let castTitle { ContinueWatchingAction(title: castTitle) }
            Button(LocalizedStringKey(asset.watched ? "Mark as Unwatched" : "Mark as Watched")) {
                saving = true
                Task {
                    defer { saving = false }
                    do {
                        let _: Store.Empty = try await store.request("POST", "/v1/library/assets/\(asset.id)/watched", body: ["watched": !asset.watched])
                        await store.refresh()
                    } catch { store.actionError = error.localizedDescription }
                }
            }.disabled(saving)
            #if !os(tvOS)
            if showsDownload, asset.complete, let title = store.library?.titles.first(where: { $0.assets.contains(where: { $0.id == asset.id }) }) {
                DownloadAssetControl(asset: asset, title: title)
            }
            #endif
            if asset.complete {
                NavigationLink { LibraryFileManagement(assetID: asset.id) } label: {
                    Label("Manage Files", systemImage: "folder")
                }
            }
            if let castTitle { LibraryCastLink(title: castTitle) }
        }
    }
}

struct LibraryEpisodeActions: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    let asset: LibraryAsset
    let title: LibraryTitle
    @State private var saving = false
    var body: some View {
        HStack(spacing: 4) {
            #if !os(tvOS)
            DownloadAssetControl(asset: asset, title: title, compact: true)
            #endif
            Button {
                saving = true
                Task {
                    defer { saving = false }
                    do {
                        let _: Store.Empty = try await store.request("POST", "/v1/library/assets/\(asset.id)/watched", body: ["watched": !asset.watched])
                        await store.refresh()
                    } catch { store.actionError = error.localizedDescription }
                }
            } label: {
                Image(systemName: asset.watched ? "checkmark.circle.fill" : "checkmark.circle").font(.title2).frame(width: 44, height: 44)
            }.disabled(saving || !store.connected)
                .accessibilityLabel(asset.watched ? "Mark as unwatched" : "Mark as watched")
                .help(asset.watched ? "Mark as unwatched" : "Mark as watched")
        }.buttonStyle(.borderless).foregroundStyle(Theme.accent)
    }
}

struct LibraryOperationStatus: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    var search = ""
    var body: some View {
        if let error = store.libraryOperationsError {
            Text("Couldn’t refresh file operations: " + error).font(.callout).foregroundStyle(.orange)
        }
        ForEach(store.detachedLibraryOperations.filter { search.isEmpty || $0.name.localizedCaseInsensitiveContains(search) }) { operation in
            VStack(alignment: .leading, spacing: 10) {
                Text(operation.name).font(.headline)
                Text(LocalizedStringKey(operation.label)).foregroundStyle(Theme.muted)
                if let error = operation.error, !error.isEmpty {
                    Text(L10n.text(error)).foregroundStyle(.orange)
                    Text("The server will retry automatically.").font(.caption).foregroundStyle(Theme.muted)
                }
            }.frame(maxWidth: .infinity, alignment: .leading).padding(20)
                .background(Theme.panel, in: RoundedRectangle(cornerRadius: 10))
                #if os(tvOS)
                .focusable()
                #endif
        }
    }
}

private struct LibraryFilePreview: Decodable {
    struct Asset: Decodable, Identifiable {
        let assetId: String
        let name: String?
        let season: Int?
        let episode: Int?
        var id: String { assetId }
        var label: String {
            let prefix = season.flatMap { s in episode.map { EpisodeNumber(season: s, episode: $0).label + " · " } } ?? ""
            return prefix + (name ?? "Media file")
        }
    }
    let name: String
    let scope: String
    let totalBytes: Int64
    let assets: [Asset]
    let allowedActions: [String]
    let blockedReasons: [String]
    let confirmationToken: String
}
private struct LibraryFileOperation: Decodable {
    let id: String
    let phase: String
    let error: String?
}
private struct LibraryFileOperations: Decodable { let operations: [LibraryFileOperation] }

struct LibraryFileManagement: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    let assetID: String
    @State private var preview: LibraryFilePreview?
    @State private var operation: LibraryFileOperation?
    @State private var pendingAction: String?
    @State private var submitting = false
    @State private var error: String?
    @State private var refreshID = UUID()
    private var path: String { "/v1/library/assets/\(assetID)/manage-files" }
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 22) {
                if let operation {
                    Text(LocalizedStringKey(operation.phase == "done" ? "Files removed" : operation.phase == "seeding" ? "In Junk · finishing seeding" : "Removing from library…")).font(.title2.bold())
                    if let failure = operation.error { Text(failure).foregroundStyle(.red) }
                    if operation.phase != "done" {
                        Text("The server will continue this operation after you leave this screen.")
                        if operation.phase != "seeding" { ProgressView() }
                    }
                } else if let preview {
                    Text(preview.name).font(.title2.bold())
                    Text(LocalizedStringKey(preview.scope == "entire_download" ? "This affects the entire download package and everyone’s library." : "This removes the file from everyone’s library."))
                    Text(ByteCountFormatter.string(fromByteCount: preview.totalBytes, countStyle: .file)).foregroundStyle(.secondary)
                    VStack(alignment: .leading, spacing: 0) {
                        ForEach(preview.assets) { asset in
                            HStack(spacing: 12) {
                                Image(systemName: "film").foregroundStyle(Theme.accent)
                                Text(asset.label).font(.subheadline).lineLimit(3)
                                Spacer(minLength: 0)
                            }.padding(16)
                            if asset.id != preview.assets.last?.id { Divider().padding(.leading, 48) }
                        }
                    }.background(Theme.panel, in: RoundedRectangle(cornerRadius: 10))
                    Text("Personal watch history and downloaded copies on other devices are kept.").foregroundStyle(.secondary)
                    ForEach(preview.blockedReasons, id: \.self) { Text($0).foregroundStyle(.red) }
                    if preview.blockedReasons.isEmpty {
                        if preview.allowedActions.contains("junk") {
                            Button { pendingAction = "junk" } label: {
                                Label("Move to Junk", systemImage: "archivebox").frame(maxWidth: .infinity).padding(10)
                            }.buttonStyle(MonoButtonStyle()).disabled(submitting)
                            Text("Finishes seeding, then deletes the files.").font(.caption).foregroundStyle(Theme.muted)
                        }
                        if preview.allowedActions.contains("delete") {
                            Button(role: .destructive) { pendingAction = "delete" } label: {
                                Label("Delete Permanently", systemImage: "trash").frame(maxWidth: .infinity).padding(10)
                            }.buttonStyle(MonoButtonStyle()).disabled(submitting)
                        }
                    }
                    if submitting { ProgressView("Submitting…") }
                } else if error == nil { ProgressView("Checking files…") }
                if let error { Text(L10n.text(error)).foregroundStyle(.red) }
                if error != nil || preview?.blockedReasons.isEmpty == false {
                    Button("Check again") { refreshID = UUID() }.disabled(submitting)
                }
            }.padding(24).frame(maxWidth: 720, alignment: .leading).frame(maxWidth: .infinity)
        }.background(Theme.canvas).navigationTitle("Manage Files")
        .confirmationDialog(pendingAction == "junk" ? "Move the entire package to Junk?" : "Permanently delete these files?",
                            isPresented: Binding(get: { pendingAction != nil }, set: { if !$0 { pendingAction = nil } }), titleVisibility: .visible) {
            let action = pendingAction ?? "delete"
            Button(LocalizedStringKey(action == "junk" ? "Move to Junk" : "Delete permanently"), role: .destructive) { submit(action) }
            Button("Cancel", role: .cancel) { pendingAction = nil }
        } message: {
            Text(LocalizedStringKey(pendingAction == "junk" ? "The entire package will be hidden from everyone, continue seeding, and be permanently deleted when seeding finishes." : "Seeding will stop. All server files listed on this page will be permanently deleted for everyone. This cannot be undone."))
        }
        .task(id: refreshID) {
            error = nil
            guard operation == nil else { return }
            do { preview = try await store.request("GET", path) }
            catch { if !Task.isCancelled { self.error = error.localizedDescription } }
        }
        .task(id: operation?.id) {
            guard let id = operation?.id else { return }
            while !Task.isCancelled && operation?.phase != "done" {
                do {
                    try await Task.sleep(for: .seconds(3))
                    let result: LibraryFileOperations = try await store.request("GET", "/v1/library/operations")
                    guard let latest = result.operations.first(where: { $0.id == id }) else { throw APIError(message: "Operation status is unavailable. The server may still be processing it.") }
                    operation = latest
                    error = nil
                    if latest.phase == "done" { await store.refresh() }
                } catch {
                    if Task.isCancelled { return }
                    self.error = error.localizedDescription
                }
            }
        }
    }
    private func submit(_ action: String) {
        guard let preview, !submitting else { return }
        submitting = true; error = nil
        Task {
            defer { submitting = false }
            do {
                operation = try await store.request("POST", path, body: ["action": action, "confirmation_token": preview.confirmationToken])
                await store.refresh()
            } catch { self.error = error.localizedDescription }
        }
    }
}

/// Disposable browsing data. SQLite snapshots are profile-scoped; image files are
/// shared by profiles on the same connection. No credentials or video are stored.
actor LibraryDiskCache {
    static let shared = LibraryDiskCache()
    private let root: URL
    private var database: OpaquePointer?
    private var lastTrim = Date.distantPast
    private let transient = unsafeBitCast(-1, to: sqlite3_destructor_type.self)
    init(root override: URL? = nil) {
        root = override ?? FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("TorrbabyLibrary-v1", isDirectory: true)
    }
    private func digest(_ value: String) -> String {
        SHA256.hash(data: Data(value.utf8)).map { String(format: "%02x", $0) }.joined()
    }
    private func open() -> Bool {
        if database != nil { return true }
        do { try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true) }
        catch { return false }
        guard sqlite3_open(root.appendingPathComponent("library.sqlite").path, &database) == SQLITE_OK else {
            sqlite3_close(database); database = nil; return false
        }
        sqlite3_busy_timeout(database, 1000)
        return sqlite3_exec(database, "CREATE TABLE IF NOT EXISTS snapshots(scope TEXT PRIMARY KEY, payload BLOB NOT NULL, updated REAL NOT NULL)", nil, nil, nil) == SQLITE_OK
    }
    func library(scope: String) -> MediaLibrary? {
        guard open() else { return nil }
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(database, "SELECT payload FROM snapshots WHERE scope=?", -1, &statement, nil) == SQLITE_OK else { return nil }
        defer { sqlite3_finalize(statement) }
        sqlite3_bind_text(statement, 1, digest(scope), -1, transient)
        guard sqlite3_step(statement) == SQLITE_ROW,
              let bytes = sqlite3_column_blob(statement, 0) else { return nil }
        let size = Int(sqlite3_column_bytes(statement, 0))
        guard size > 0, size <= 16 * 1024 * 1024 else { return nil }
        let decoder = JSONDecoder(); decoder.keyDecodingStrategy = .convertFromSnakeCase
        return try? decoder.decode(MediaLibrary.self, from: Data(bytes: bytes, count: size))
    }
    func saveLibrary(_ payload: Data, scope: String) {
        guard payload.count <= 16 * 1024 * 1024, open() else { return }
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(database, "INSERT OR REPLACE INTO snapshots VALUES(?,?,?)", -1, &statement, nil) == SQLITE_OK else { return }
        defer { sqlite3_finalize(statement) }
        sqlite3_bind_text(statement, 1, digest(scope), -1, transient)
        _ = payload.withUnsafeBytes { sqlite3_bind_blob(statement, 2, $0.baseAddress, Int32($0.count), transient) }
        sqlite3_bind_double(statement, 3, Date().timeIntervalSince1970)
        guard sqlite3_step(statement) == SQLITE_DONE else { return }
        sqlite3_exec(database, "DELETE FROM snapshots WHERE scope NOT IN (SELECT scope FROM snapshots ORDER BY updated DESC LIMIT 8)", nil, nil, nil)
    }
    func artwork(key: String) -> Data? {
        let path = root.appendingPathComponent(digest(key) + ".image")
        guard let values = try? path.resourceValues(forKeys: [.fileSizeKey, .contentModificationDateKey]),
              (values.fileSize ?? 0) <= 8 * 1024 * 1024 else { return nil }
        return try? Data(contentsOf: path)
    }
    func saveArtwork(_ data: Data, key: String) {
        guard data.count <= 8 * 1024 * 1024 else { return }
        do {
            try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
            try data.write(to: root.appendingPathComponent(digest(key) + ".image"), options: .atomic)
            guard Date().timeIntervalSince(lastTrim) > 60 else { return }
            lastTrim = Date()
            let urls = try FileManager.default.contentsOfDirectory(at: root, includingPropertiesForKeys: [.fileSizeKey, .contentModificationDateKey])
            let entries = urls.filter { $0.pathExtension == "image" }.compactMap { url -> (URL, Int, Date)? in
                guard let v = try? url.resourceValues(forKeys: [.fileSizeKey, .contentModificationDateKey]) else { return nil }
                return (url, v.fileSize ?? 0, v.contentModificationDate ?? .distantPast)
            }.sorted { $0.2 < $1.2 }
            var total = entries.reduce(0) { $0 + $1.1 }
            for (url, size, _) in entries where total > 192 * 1024 * 1024 {
                try FileManager.default.removeItem(at: url); total -= size
            }
        } catch { /* Cache failure must never prevent browsing. */ }
    }
}

struct ContinueWatchingAction: View {
    @EnvironmentObject var store: Store
    let title: LibraryTitle
    var body: some View {
        if title.isContinuing {
            Button("Remove from Continue Watching") {
                Task { await store.setContinueWatchingHidden(title.id, hidden: true) }
            }
        }
    }
}
struct ContinueWatchingUndoBanner: View {
    @EnvironmentObject var store: Store
    var body: some View {
        if let undo = store.continueWatchingUndo, undo.profile == store.profileIdentity {
            HStack(spacing: 16) {
                Text("Removed from Continue Watching")
                Button("Undo") { Task { await store.setContinueWatchingHidden(undo.id, hidden: false) } }
                Button { store.continueWatchingUndo = nil } label: { Image(systemName: "xmark") }
                    .accessibilityLabel("Dismiss")
            }.padding(16).background(.regularMaterial, in: RoundedRectangle(cornerRadius: 10)).padding()
                .task(id: undo.id) {
                    do { try await Task.sleep(for: .seconds(10)) } catch { return }
                    if store.continueWatchingUndo?.id == undo.id { store.continueWatchingUndo = nil }
                }
        }
    }
}

/// Wrap action buttons at their natural widths with consistent spacing.
struct LibraryActionFlow: Layout {
    var spacing: CGFloat = 10
    private func layout(_ subviews: Subviews, width: CGFloat) -> (CGSize, [CGPoint]) {
        var x: CGFloat = 0, y: CGFloat = 0, height: CGFloat = 0
        var points: [CGPoint] = []
        var used: CGFloat = 0
        for view in subviews {
            let size = view.sizeThatFits(.unspecified)
            if x > 0 && x + size.width > width { x = 0; y += height + spacing; height = 0 }
            points.append(CGPoint(x: x, y: y))
            used = max(used, x + size.width)
            x += size.width + spacing; height = max(height, size.height)
        }
        return (CGSize(width: used, height: y + height), points)
    }
    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        layout(subviews, width: proposal.width ?? .infinity).0
    }
    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        let points = layout(subviews, width: bounds.width).1
        for (view, point) in zip(subviews, points) {
            view.place(at: CGPoint(x: bounds.minX + point.x, y: bounds.minY + point.y), proposal: .unspecified)
        }
    }
}
