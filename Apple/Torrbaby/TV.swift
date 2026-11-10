#if os(tvOS)
import SwiftUI
import OSLog

@MainActor
private final class TVDiscovery: NSObject, ObservableObject, @preconcurrency NetServiceBrowserDelegate, @preconcurrency NetServiceDelegate {
    @Published var addresses: [String] = []
    private let browser = NetServiceBrowser()
    private var services: [NetService] = []
    func start() {
        browser.delegate = self
        browser.searchForServices(ofType: "_torrbaby._tcp.", inDomain: "local.")
    }
    func stop() { browser.stop(); services.forEach { $0.stop() }; services.removeAll() }
    func netServiceBrowser(_ browser: NetServiceBrowser, didFind service: NetService, moreComing: Bool) {
        services.append(service); service.delegate = self; service.resolve(withTimeout: 10)
    }
    func netServiceDidResolveAddress(_ sender: NetService) {
        guard let host = sender.hostName, sender.port > 0 else { return }
        var parts = URLComponents(); parts.scheme = "http"; parts.host = host; parts.port = sender.port
        if let value = parts.url?.absoluteString, !addresses.contains(value) { addresses.append(value) }
    }
}

private struct TVConnectionView: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    @StateObject private var discovery = TVDiscovery()
    @State private var address = ""
    @State private var manual = false
    @State private var busy = false
    @State private var error: String?
    @State private var attempted: Set<String> = []
    var done: () -> Void
    var body: some View {
        VStack(spacing: 28) {
            Image("TorrbabyLogo").resizable().frame(width: 120, height: 120).clipShape(RoundedRectangle(cornerRadius: 24))
            Text("Welcome home").font(.largeTitle.bold())
            if busy { ProgressView("Connecting…") }
            else { Label("Finding your library…", systemImage: "wifi") }
            if let error { Text(L10n.text(error)).font(.callout).foregroundStyle(.orange) }
            ForEach(discovery.addresses, id: \.self) { value in
                Button(value) { Task { await connect(value) } }.disabled(busy)
            }
            Button("Enter server address") { manual.toggle() }
            if manual {
                TextField("http://diamond.local:8765", text: $address)
                Button("Connect") { Task { await connect(address) } }.disabled(busy || address.isEmpty)
            }
            if store.configured { Button("Back to library", action: done) }
            Text(AppRelease.description).font(.caption).foregroundStyle(.secondary)
        }.frame(maxWidth: 850)
        .onAppear { discovery.start() }
        .onDisappear { discovery.stop() }
        .task(id: discovery.addresses) {
            guard !store.configured, let first = discovery.addresses.first, !attempted.contains(first) else { return }
            attempted.insert(first); await connect(first)
        }
    }
    private func connect(_ value: String) async {
        guard !busy else { return }; busy = true; error = nil
        defer { busy = false }
        do {
            let base = value.trimmingCharacters(in: .whitespacesAndNewlines).trimmingCharacters(in: CharacterSet(charactersIn: "/"))
            guard let url = URL(string: base + "/v1/lan/register"), ["http", "https"].contains(url.scheme ?? ""), url.host != nil else { throw URLError(.badURL) }
            var request = URLRequest(url: url); request.httpMethod = "POST"; request.timeoutInterval = 15
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = try JSONSerialization.data(withJSONObject: ["name": store.defaultClientName, "platform": "tvOS"])
            let (data, response) = try await URLSession.shared.data(for: request)
            guard (response as? HTTPURLResponse)?.statusCode == 200 else { throw URLError(.userAuthenticationRequired) }
            struct Registration: Decodable { let client_id: String; let token: String }
            let registration = try JSONDecoder().decode(Registration.self, from: data)
            try Credentials.save(registration.client_id, account: "client-id")
            try Credentials.save(registration.token)
            store.clientId = registration.client_id; store.server = base; store.token = registration.token
            await store.connect()
            if store.connected { done() } else { error = "Couldn’t open this library. Try again." }
        } catch {
            let failure = error as NSError
            Logger(subsystem: "org.torrbaby.tv", category: "connection").error("LAN connection failed: domain=\(failure.domain, privacy: .public) code=\(failure.code)")
            self.error = "Couldn’t connect. Check that your TV and server are on the same network."
        }
    }
}

struct TVRootView: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    @Environment(\.scenePhase) var phase
    @State private var settings = false
    @State private var showSubtitles = false
    var body: some View {
        NavigationStack {
            if !store.configured || settings {
                TVConnectionView { settings = false }
            } else {
                TabView {
                    TVLibraryView(kind: nil).tabItem { Label("Home", systemImage: "house") }
                    TVLibraryView(kind: "movie").tabItem { Label("Movies", systemImage: "film") }
                    TVLibraryView(kind: "tv").tabItem { Label("TV Shows", systemImage: "tv") }
                    TorrentSearchView().tabItem { Label("Find torrents", systemImage: "magnifyingglass") }
                    TVTorrentView().tabItem { Label("Torrents", systemImage: "arrow.down.circle") }
                    TVSearchView().tabItem { Label("Search", systemImage: "magnifyingglass") }
                    Button("Switch user") { store.choosingProfile = true }
                        .tabItem { Label(store.activeProfile?.name ?? "User", systemImage: "person.crop.circle") }
                    VStack(spacing: 30) {
                        Text("Torrbaby").font(.largeTitle.bold())
                        Text(AppRelease.description).font(.headline).monospacedDigit().foregroundStyle(.secondary)
                        Image("TorrbabyLogo").resizable().frame(width: 120, height: 120)
                        Label(LocalizedStringKey(store.connected ? "Connected" : "Offline"), systemImage: store.connected ? "checkmark.circle" : "wifi.slash")
                        Button("Default subtitles") { showSubtitles = true }
                            .sheet(isPresented: $showSubtitles) { LocalizedInterface { SubtitlePreferencesView() } }
                        BackgroundSubtitlesView().frame(maxWidth: 850)
                        Button("Connection settings") { settings = true }
                    }.tabItem { Label("Settings", systemImage: "gearshape") }
                }
                .navigationDestination(for: LibraryDestination.self) { route in TVMediaDetail(titleID: route.id) }
            }
        }
        .overlay(alignment: .bottom) { ContinueWatchingUndoBanner() }
        .task(id: phase) {
            while !Task.isCancelled && phase == .active {
                await store.refresh()
                try? await Task.sleep(for: .seconds(10))
            }
        }
        .alert("Couldn’t complete the request", isPresented: Binding(get: { store.actionError != nil }, set: { if !$0 { store.actionError = nil } })) {
            Button("OK") { store.actionError = nil }
        } message: { Text(LocalizedStringKey(store.actionError ?? "")) }
    }
}

private struct TVLibraryView: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    let kind: String?
    private var titles: [LibraryTitle] {
        (store.library?.titles ?? []).filter { kind == nil || $0.kind == kind }
            .sorted { $0.addedAt > $1.addedAt }
    }
    private var featured: LibraryTitle? { titles.homeHero }
    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 48) {
                if kind == nil, let featured {
                    NavigationLink(value: LibraryDestination(id: featured.id)) {
                        LibraryArtwork(id: featured.id, variant: "backdrop")
                            .frame(height: 600)
                            .overlay(LinearGradient(colors: [.black.opacity(0.1), .black.opacity(0.9)], startPoint: .top, endPoint: .bottom))
                            .overlay(alignment: .bottomLeading) {
                                VStack(alignment: .leading, spacing: 16) {
                                    Text(featured.title).font(.system(size: 48, weight: .bold)).lineLimit(2)
                                    Text(LocalizedStringKey(([featured.year ?? ""] + featured.genres.prefix(2)).filter { !$0.isEmpty }.joined(separator: " · ")))
                                        .font(.headline)
                                    Label("Explore", systemImage: "play.circle.fill").font(.headline)
                                }.foregroundStyle(.white).padding(.horizontal, 90).padding(.bottom, 55)
                                    .frame(maxWidth: 1250, alignment: .leading)
                            }
                    }.buttonStyle(TVHeroButtonStyle()).padding(.horizontal, -70).padding(.top, -40)
                }
                if kind == nil {
                    let continuing = titles.filter { $0.isContinuing }.sorted { $0.continueWatchingAt > $1.continueWatchingAt }
                    TVShelf(name: "Continue watching", titles: continuing)
                    TVShelf(name: "Recently added", titles: titles)
                    let watched = titles.filter { $0.assets.contains { $0.lastPlayed > 0 } }
                        .sorted { ($0.assets.map(\.lastPlayed).max() ?? 0) > ($1.assets.map(\.lastPlayed).max() ?? 0) }
                    TVShelf(name: "Recently watched", titles: watched)
                    TVShelf(name: "Movies", titles: titles.filter { $0.kind == "movie" })
                    TVShelf(name: "TV Shows", titles: titles.filter { $0.kind == "tv" })
                    let genres = Array(Set(titles.flatMap(\.genres))).sorted()
                    ForEach(genres, id: \.self) { genre in
                        TVShelf(name: genre, titles: titles.filter { $0.genres.contains(genre) })
                    }
                } else {
                    Text(LocalizedStringKey(kind == "movie" ? "Movies" : "TV Shows")).font(.largeTitle.bold())
                    TVPosterGrid(titles: titles.sorted { $0.title.localizedStandardCompare($1.title) == .orderedAscending })
                }
                if titles.isEmpty { ContentUnavailableView("Your library", systemImage: "film.stack") }
                if let error = store.libraryError { Text(L10n.text(error)).foregroundStyle(.orange) }
            }.padding(.horizontal, 70).padding(.vertical, 40)
        }.ignoresSafeArea(.container, edges: .horizontal).background {
            GeometryReader { geometry in
                if let featured {
                    LibraryArtwork(id: featured.id, variant: "backdrop")
                        .frame(width: geometry.size.width, height: geometry.size.height)
                        .blur(radius: 90).saturation(0.65).opacity(0.38)
                        .overlay(Theme.canvas.opacity(0.4))
                }
            }.background(Theme.canvas).ignoresSafeArea()
        }
    }
}

private struct TVHeroButtonStyle: ButtonStyle {
    @Environment(\.isFocused) private var focused
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .overlay(alignment: .bottom) {
                Rectangle().fill(Theme.mint).frame(height: focused ? 5 : 0)
            }
            .brightness(configuration.isPressed ? -0.08 : 0)
    }
}

private struct TVMediaDetail: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    let titleID: String
    @State private var season: Int?
    @State private var playback: Playback?
    @State private var loading = false
    @State private var preparation = ""
    @State private var playbackTask: Task<Void, Never>?
    private var title: LibraryTitle? { store.library?.titles.first { $0.id == titleID } }
    var body: some View {
        ScrollView {
            if let title {
                VStack(alignment: .leading, spacing: 40) {
                    ZStack(alignment: .bottomLeading) {
                        LibraryArtwork(id: title.id, variant: title.backdrop == nil ? "poster" : "backdrop")
                        LinearGradient(colors: [.clear, .black.opacity(0.95)], startPoint: .top, endPoint: .bottom)
                        VStack(alignment: .leading, spacing: 22) {
                            Text(title.title).font(.system(size: 56, weight: .bold)).lineLimit(2)
                            Text(LocalizedStringKey(([title.year ?? "", title.contentRating ?? ""] + title.genres.prefix(3)).filter { !$0.isEmpty }.joined(separator: " · ")))
                                .font(.headline).foregroundStyle(.white.opacity(0.8))
                            if let asset = title.resume ?? title.playable.first(where: { !$0.watched }) ?? title.playable.first {
                                Button { play(asset, title: title) } label: {
                                    Label(asset.playbackActionTitle(television: title.kind == "tv"), systemImage: "play.fill")
                                        .frame(width: title.kind == "tv" ? 650 : nil, alignment: .leading)
                                }.buttonStyle(.borderedProminent).tint(Theme.mint).disabled(loading)
                                LibraryAssetActions(asset: asset, castTitle: title)
                            }
                        }.padding(.horizontal, 90).padding(.bottom, 50).frame(maxWidth: 1350, alignment: .leading)
                    }.frame(height: 650)
                    VStack(alignment: .leading, spacing: 38) {
                        if loading {
                            HStack {
                                Text(LocalizedStringKey(preparation.isEmpty ? "Preparing playback…" : preparation))
                                Button("Cancel") { playbackTask?.cancel() }
                            }
                        }
                        if let asset = title.resume ?? title.playable.first(where: { !$0.watched }) ?? title.playable.first {
                            MediaLanguagesView(info: asset.mediaInfo)
                        }
                        if !title.summary.isEmpty {
                            Text(title.summary).font(.system(size: 27)).lineSpacing(7).frame(maxWidth: 1300, alignment: .leading)
                        }
                        let seasons = Array(Set(title.assets.filter { $0.role != "extra" }.compactMap(\.season))).sorted()
                        let selectedSeason = season ?? title.resume?.season ?? title.playable.first(where: { !$0.watched && ($0.season ?? 0) > 0 })?.season ?? seasons.first(where: { $0 > 0 }) ?? seasons.first
                        if title.kind == "tv" {
                            ScrollView(.horizontal) {
                                HStack(spacing: 25) {
                                    ForEach(seasons, id: \.self) { number in
                                        Button(LocalizedStringKey(number == 0 ? "Specials" : "Season \(number)")) { season = number }
                                            .tint(selectedSeason == number ? Theme.mint : .gray)
                                    }
                                }.padding(15)
                            }.scrollClipDisabled().focusSection()
                        }
                        let assets = title.assets.filter { $0.role != "extra" && (title.kind != "tv" || $0.season == selectedSeason) }
                            .sorted { ($0.episode ?? 0, $0.name ?? "") < ($1.episode ?? 0, $1.name ?? "") }
                        if title.kind == "tv" || assets.count > 1 {
                            ForEach(assets) { asset in assetButton(asset, title: title) }
                        }
                        let unnumbered = title.assets.filter { $0.role != "extra" && $0.season == nil }
                        if title.kind == "tv" && selectedSeason != nil && !unnumbered.isEmpty {
                            Text("Other episodes").font(.title2.bold())
                            ForEach(unnumbered) { asset in assetButton(asset, title: title) }
                        }
                        let extras = title.assets.filter { $0.role == "extra" }
                        if !extras.isEmpty {
                            Text("Special features").font(.title2.bold())
                            ForEach(extras) { asset in assetButton(asset, title: title) }
                        }
                    }.padding(.horizontal, 90).padding(.bottom, 70)
                }
            }
        }.ignoresSafeArea(.container, edges: [.top, .horizontal])
            .background {
                if let title {
                    LibraryArtwork(id: title.id, variant: title.backdrop == nil ? "poster" : "backdrop")
                        .blur(radius: 90).opacity(0.25).overlay(Theme.canvas.opacity(0.6)).ignoresSafeArea()
                }
            }
            .fullScreenCover(item: $playback) { PlayerScreen(playback: $0, store: store).onExitCommand { playback = nil } }
            .onDisappear { playbackTask?.cancel() }
    }
    private func assetButton(_ asset: LibraryAsset, title: LibraryTitle) -> some View {
        HStack(spacing: 20) {
        Button { play(asset, title: title) } label: {
            HStack(spacing: 28) {
                if asset.thumbnail != nil {
                    LibraryArtwork(id: asset.id, variant: "episode")
                        .frame(width: 250, height: 140).clipShape(RoundedRectangle(cornerRadius: 10))
                }
                Image(systemName: asset.watched ? "checkmark.circle.fill" : "play.circle.fill").font(.largeTitle)
                VStack(alignment: .leading, spacing: 10) {
                    if let episode = asset.episode { Text("Episode \(episode)").font(.caption) }
                    Text(asset.name ?? title.title).font(.headline).lineLimit(2)
                    if !asset.summary.isEmpty { Text(asset.summary).font(.caption).lineLimit(2) }
                    if !asset.complete { Text("Downloading").font(.caption).foregroundStyle(Theme.muted) }
                    if asset.position > 0 && !asset.watched && asset.duration > 0 {
                        ProgressView(value: min(1, asset.position / asset.duration)).tint(Theme.mint)
                    }
                }
                Spacer()
                if asset.duration > 0 { Text("\(Int(asset.duration / 60)) min").font(.caption) }
            }.padding(28).frame(maxWidth: .infinity, alignment: .leading)
        }.buttonStyle(.card).disabled(!asset.complete || loading)
        LibraryEpisodeActions(asset: asset, title: title)
        }
    }
    private func play(_ asset: LibraryAsset, title: LibraryTitle) {
        guard !loading else { return }
        loading = true; preparation = ""
        playbackTask = Task {
            defer { loading = false; playbackTask = nil }
            do {
                await store.flushProgress()
                try await store.registerClient()
                let link = try await store.mediaPlaybackLink(asset: asset, titleID: title.id) { preparation = $0 }
                playback = Playback(sessionId: link.sessionId, resumePosition: link.resumePosition ?? 0,
                    url: try store.endpoint(link.path), name: asset.episode == nil ? title.title : title.title + " · " + (asset.name ?? "Episode \(asset.episode!)"), libraryAssetID: asset.id, libraryTitleID: title.id)
            } catch { if !Task.isCancelled { store.actionError = error.localizedDescription } }
        }
    }
}

private struct TVShelf: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    let name: String
    let titles: [LibraryTitle]
    var body: some View {
        if !titles.isEmpty {
            VStack(alignment: .leading, spacing: 24) {
                HStack {
                    Text(L10n.text(name)).font(.title2.bold())
                    Spacer()
                    NavigationLink {
                        TVCollectionView(name: name, titleIDs: titles.map(\.id))
                    } label: { Label("See All", systemImage: "chevron.right").font(.headline) }
                }
                ScrollView(.horizontal) {
                    LazyHStack(alignment: .top, spacing: 40) {
                        ForEach(Array(titles.prefix(10))) { TVPoster(title: $0).frame(width: 240) }
                    }.padding(20)
                }.scrollClipDisabled().scrollIndicators(.hidden)
            }.focusSection()
        }
    }
}


private struct TVCollectionView: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    let name: String
    let titleIDs: [String]
    private var titles: [LibraryTitle] {
        let byID = Dictionary((store.library?.titles ?? []).map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        return titleIDs.compactMap { byID[$0] }
    }
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 40) {
                Text(L10n.text(name)).font(.largeTitle.bold())
                TVPosterGrid(titles: titles)
            }.padding(70)
        }.background(Theme.canvas)
    }
}

private struct TVPoster: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    let title: LibraryTitle
    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            NavigationLink(value: LibraryDestination(id: title.id)) {
                LibraryArtwork(id: title.id).aspectRatio(2.0 / 3.0, contentMode: .fit)
                    .clipShape(RoundedRectangle(cornerRadius: 16))
                    .overlay(alignment: .topTrailing) { if title.watched { WatchedBadge() } }
            }.buttonStyle(.card).contextMenu { ContinueWatchingAction(title: title) }
            Text(title.title).font(.system(size: 25, weight: .semibold)).lineLimit(2)
            Text(LocalizedStringKey(title.continuationLabel ?? title.year ?? "")).font(.system(size: 21)).foregroundStyle(Theme.muted)
        }
    }
}

private struct TVPosterGrid: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    let titles: [LibraryTitle]
    var body: some View {
        LazyVGrid(columns: [GridItem(.adaptive(minimum: 240, maximum: 300), spacing: 40)], alignment: .leading, spacing: 40) {
            ForEach(titles) { TVPoster(title: $0) }
        }.focusSection()
    }
}

private struct TVTorrentView: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    @State private var category = "Downloading"
    @AppStorage("torrentSort.downloading") private var downloadingSort = TorrentSort.progress
    @AppStorage("torrentSort.seeding") private var seedingSort = TorrentSort.ratio
    @AppStorage("torrentSort.complete") private var completeSort = TorrentSort.alphabetic
    private var sortSelection: Binding<TorrentSort> {
        switch category {
        case "Seeding": $seedingSort
        case "Complete": $completeSort
        default: $downloadingSort
        }
    }
    private var torrents: [Torrent] {
        sortSelection.wrappedValue.ordered(store.torrents.filter {
            if $0.fileOperation != nil { return category == "Seeding" }
            return category == "Downloading" ? !$0.completed : category == "Seeding" ? $0.completed && $0.status == 6 : $0.completed && $0.status != 6
        })
    }
    var body: some View {
        VStack(spacing: 30) {
            Picker("Torrents", selection: $category) {
                ForEach(["Downloading", "Seeding", "Complete"], id: \.self) { Text(LocalizedStringKey($0)).tag($0) }
            }.pickerStyle(.segmented).padding(.horizontal, 70)
            if let error = store.torrentError {
                Text("Couldn’t refresh torrents: " + error).font(.caption).foregroundStyle(.orange).padding(.horizontal, 70)
            }
            Picker("Sort", selection: sortSelection) {
                ForEach(TorrentSort.choices(category)) { Text(LocalizedStringKey($0.label)).tag($0) }
            }.pickerStyle(.segmented).padding(.horizontal, 70)
            ScrollView {
                LazyVStack(spacing: 28) {
                    if category == "Seeding" { LibraryOperationStatus() }
                    ForEach(torrents) { torrent in
                        VStack(alignment: .leading, spacing: 18) {
                            Text(torrent.displayName).font(.headline).lineLimit(2)
                            if let operation = torrent.fileOperation {
                                Text(LocalizedStringKey(operation.label)).font(.caption).foregroundStyle(operation.error == nil ? Color.secondary : Color.red)
                            }
                            HStack(spacing: 32) {
                                Text(bytes(torrent.size))
                                Label(rate(torrent.rateDownload), systemImage: "arrow.down").foregroundStyle(Theme.mint)
                                Label(rate(torrent.rateUpload), systemImage: "arrow.up").foregroundStyle(.cyan)
                                Text("Ratio \(max(0, torrent.uploadRatio), specifier: "%.2f")")
                            }.font(.caption).monospacedDigit()
                            if !torrent.completed {
                                GeometryReader { geometry in
                                    ZStack(alignment: .leading) {
                                        Color.purple.opacity(0.2)
                                        Color(red: 0.70, green: 0.43, blue: 1.0).frame(width: geometry.size.width * min(1, max(0, torrent.percentDone)))
                                        Text(torrent.percentDone, format: .percent.precision(.fractionLength(0)))
                                            .font(.system(size: 20, weight: .bold)).frame(maxWidth: .infinity)
                                    }.clipShape(RoundedRectangle(cornerRadius: 8))
                                }.frame(height: 30)
                            }
                        }.padding(28).frame(maxWidth: .infinity, alignment: .leading)
                            .background(Theme.panel, in: RoundedRectangle(cornerRadius: 18)).focusable()
                    }
                    if torrents.isEmpty && (category != "Seeding" || store.detachedLibraryOperations.isEmpty) { ContentUnavailableView("No torrents", systemImage: "arrow.down.circle") }
                }.padding(70)
            }
        }.background(Theme.canvas)
    }
}

private struct TVSearchView: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    @State private var query = ""
    private var results: [LibraryTitle] {
        guard !query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return [] }
        return (store.library?.titles ?? []).filter { $0.title.localizedCaseInsensitiveContains(query) }
    }
    var body: some View {
        ScrollView {
            TVPosterGrid(titles: results).padding(70)
        }.searchable(text: $query, prompt: "Movies and TV shows")
            .background(Theme.canvas)
    }
}
#endif
