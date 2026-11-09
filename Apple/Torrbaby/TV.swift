#if os(tvOS)
import SwiftUI

struct TVRootView: View {
    @EnvironmentObject var store: Store
    @Environment(\.scenePhase) var phase
    @State private var server = ""
    @State private var token = ""
    @State private var settings = false
    var body: some View {
        NavigationStack {
            if !store.configured || settings {
                VStack(alignment: .leading, spacing: 24) {
                    Image("TorrbabyLogo").resizable().frame(width: 110, height: 110)
                        .clipShape(RoundedRectangle(cornerRadius: 22)).accessibilityHidden(true)
                    Text("Meet your server.").font(.largeTitle.bold())
                    TextField("Server URL", text: $server)
                    SecureField("API token", text: $token)
                    Button("Connect") {
                        store.server = server; store.token = token
                        Task { await store.connect(); if store.connected { settings = false } }
                    }
                    if let error = store.connectionError { Text(error).foregroundStyle(.orange) }
                    if store.configured { Button("Back to library") { settings = false } }
                }.frame(maxWidth: 900).onAppear { server = store.server; token = store.token }
            } else {
                TabView {
                    TVLibraryView(kind: nil).tabItem { Label("Home", systemImage: "house") }
                    TVLibraryView(kind: "movie").tabItem { Label("Movies", systemImage: "film") }
                    TVLibraryView(kind: "tv").tabItem { Label("TV Shows", systemImage: "tv") }
                    TVTorrentView().tabItem { Label("Torrents", systemImage: "arrow.down.circle") }
                    TVSearchView().tabItem { Label("Search", systemImage: "magnifyingglass") }
                    VStack(spacing: 30) {
                        Image("TorrbabyLogo").resizable().frame(width: 120, height: 120)
                        Label(store.connected ? "Connected" : "Offline", systemImage: store.connected ? "checkmark.circle" : "wifi.slash")
                        Button("Connection settings") { settings = true }
                    }.tabItem { Label("Settings", systemImage: "gearshape") }
                }
                .navigationDestination(for: LibraryDestination.self) { route in TVMediaDetail(titleID: route.id) }
            }
        }
        .task(id: phase) {
            while !Task.isCancelled && phase == .active {
                await store.refresh()
                try? await Task.sleep(for: .seconds(10))
            }
        }
        .alert("Couldn’t complete the request", isPresented: Binding(get: { store.actionError != nil }, set: { if !$0 { store.actionError = nil } })) {
            Button("OK") { store.actionError = nil }
        } message: { Text(store.actionError ?? "") }
    }
}

private struct TVLibraryView: View {
    @EnvironmentObject var store: Store
    let kind: String?
    private var titles: [LibraryTitle] {
        (store.library?.titles ?? []).filter { kind == nil || $0.kind == kind }
            .sorted { $0.addedAt > $1.addedAt }
    }
    private var featured: LibraryTitle? { titles.first(where: { $0.backdrop != nil && !$0.playable.isEmpty }) }
    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 48) {
                if let featured {
                    NavigationLink(value: LibraryDestination(id: featured.id)) {
                        LibraryArtwork(id: featured.id, variant: "backdrop")
                            .frame(height: 600)
                            .overlay(LinearGradient(colors: [.black.opacity(0.1), .black.opacity(0.9)], startPoint: .top, endPoint: .bottom))
                            .overlay(alignment: .bottomLeading) {
                                VStack(alignment: .leading, spacing: 16) {
                                    Text(featured.title).font(.system(size: 48, weight: .bold)).lineLimit(2)
                                    Text(([featured.year ?? ""] + featured.genres.prefix(2)).filter { !$0.isEmpty }.joined(separator: " · "))
                                        .font(.headline)
                                    Label("Explore", systemImage: "play.circle.fill").font(.headline)
                                }.foregroundStyle(.white).padding(.horizontal, 90).padding(.bottom, 55)
                                    .frame(maxWidth: 1250, alignment: .leading)
                            }
                    }.buttonStyle(TVHeroButtonStyle()).padding(.horizontal, -70).padding(.top, -40)
                }
                let continuing = titles.filter { $0.resume != nil }.sorted { ($0.resume?.lastPlayed ?? 0) > ($1.resume?.lastPlayed ?? 0) }
                TVShelf(name: "Continue watching", titles: continuing)
                TVShelf(name: "Recently added", titles: Array(titles.prefix(20)))
                if kind == nil {
                    TVShelf(name: "Movies", titles: titles.filter { $0.kind == "movie" })
                    TVShelf(name: "TV Shows", titles: titles.filter { $0.kind == "tv" })
                } else {
                    Text(kind == "movie" ? "All movies" : "All shows").font(.title2.bold())
                    TVPosterGrid(titles: titles.sorted { $0.title.localizedStandardCompare($1.title) == .orderedAscending })
                }
                if titles.isEmpty { ContentUnavailableView("Your library", systemImage: "film.stack") }
                if let error = store.libraryError { Text(error).foregroundStyle(.orange) }
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
    @EnvironmentObject var store: Store
    let titleID: String
    @State private var season: Int?
    @State private var playback: Playback?
    @State private var loading = false
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
                            Text(([title.year ?? "", title.contentRating ?? ""] + title.genres.prefix(3)).filter { !$0.isEmpty }.joined(separator: " · "))
                                .font(.headline).foregroundStyle(.white.opacity(0.8))
                            if let asset = title.resume ?? title.playable.first(where: { !$0.watched }) ?? title.playable.first {
                                Button { play(asset, title: title) } label: {
                                    Label(asset.position > 0 && !asset.watched ? "Resume" : "Play", systemImage: "play.fill")
                                }.buttonStyle(.borderedProminent).disabled(loading)
                            }
                        }.padding(.horizontal, 90).padding(.bottom, 50).frame(maxWidth: 1350, alignment: .leading)
                    }.frame(height: 650)
                    VStack(alignment: .leading, spacing: 38) {
                        if !title.summary.isEmpty {
                            Text(title.summary).font(.system(size: 27)).lineSpacing(7).frame(maxWidth: 1300, alignment: .leading)
                        }
                        let seasons = Array(Set(title.assets.filter { $0.role != "extra" }.compactMap(\.season))).sorted()
                        let selectedSeason = season ?? title.resume?.season ?? title.playable.first(where: { !$0.watched && ($0.season ?? 0) > 0 })?.season ?? seasons.first(where: { $0 > 0 }) ?? seasons.first
                        if title.kind == "tv" {
                            ScrollView(.horizontal) {
                                HStack(spacing: 25) {
                                    ForEach(seasons, id: \.self) { number in
                                        Button(number == 0 ? "Specials" : "Season \(number)") { season = number }
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
    }
    private func assetButton(_ asset: LibraryAsset, title: LibraryTitle) -> some View {
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
    }
    private func play(_ asset: LibraryAsset, title: LibraryTitle) {
        guard !loading else { return }
        loading = true
        Task {
            defer { loading = false }
            do {
                await store.flushProgress()
                try await store.registerClient()
                let link: PlaybackLink = try await store.request("POST", "/v1/library/assets/\(asset.id)/play", body: ["client_id": store.clientId])
                playback = Playback(sessionId: link.sessionId, resumePosition: link.resumePosition ?? 0,
                    url: try store.endpoint(link.path), name: asset.episode == nil ? title.title : title.title + " · " + (asset.name ?? "Episode \(asset.episode!)"), libraryAssetID: asset.id, libraryTitleID: title.id)
            } catch { store.actionError = error.localizedDescription }
        }
    }
}

private struct TVShelf: View {
    let name: String
    let titles: [LibraryTitle]
    var body: some View {
        if !titles.isEmpty {
            VStack(alignment: .leading, spacing: 24) {
                Text(name).font(.title2.bold())
                ScrollView(.horizontal) {
                    LazyHStack(alignment: .top, spacing: 40) {
                        ForEach(titles) { TVPoster(title: $0).frame(width: 240) }
                    }.padding(20)
                }.scrollClipDisabled().scrollIndicators(.hidden)
            }.focusSection()
        }
    }
}

private struct TVPoster: View {
    let title: LibraryTitle
    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            NavigationLink(value: LibraryDestination(id: title.id)) {
                LibraryArtwork(id: title.id).aspectRatio(2.0 / 3.0, contentMode: .fit)
                    .clipShape(RoundedRectangle(cornerRadius: 16))
            }.buttonStyle(.card)
            Text(title.title).font(.system(size: 25, weight: .semibold)).lineLimit(2)
            Text(title.year ?? "").font(.system(size: 21)).foregroundStyle(Theme.muted)
        }
    }
}

private struct TVPosterGrid: View {
    let titles: [LibraryTitle]
    var body: some View {
        LazyVGrid(columns: [GridItem(.adaptive(minimum: 240, maximum: 300), spacing: 40)], alignment: .leading, spacing: 40) {
            ForEach(titles) { TVPoster(title: $0) }
        }.focusSection()
    }
}

private struct TVTorrentView: View {
    @EnvironmentObject var store: Store
    @State private var category = "Downloading"
    private var torrents: [Torrent] {
        store.torrents.filter {
            category == "Downloading" ? !$0.completed : category == "Seeding" ? $0.completed && $0.status == 6 : $0.completed && $0.status != 6
        }
    }
    var body: some View {
        VStack(spacing: 30) {
            Picker("Torrents", selection: $category) {
                ForEach(["Downloading", "Seeding", "Complete"], id: \.self) { Text($0).tag($0) }
            }.pickerStyle(.segmented).padding(.horizontal, 70)
            ScrollView {
                LazyVStack(spacing: 28) {
                    ForEach(torrents) { torrent in
                        VStack(alignment: .leading, spacing: 18) {
                            Text(torrent.displayName).font(.headline).lineLimit(2)
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
                    if torrents.isEmpty { ContentUnavailableView("No torrents", systemImage: "arrow.down.circle") }
                }.padding(70)
            }
        }.background(Theme.canvas)
    }
}

private struct TVSearchView: View {
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
