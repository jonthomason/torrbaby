#if os(tvOS)
import SwiftUI
import OSLog
import Darwin

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
        guard var host = sender.hostName, sender.port > 0 else { return }
        // Bootstrap trusts IPv4 LAN ranges. Prefer the discovered LAN address over
        // a hostname that may resolve to IPv6 or a Tailscale interface.
        for data in sender.addresses ?? [] {
            let numeric: String? = data.withUnsafeBytes { bytes in
                guard let address = bytes.baseAddress?.assumingMemoryBound(to: sockaddr.self),
                      address.pointee.sa_family == sa_family_t(AF_INET) else { return nil }
                var buffer = [CChar](repeating: 0, count: Int(NI_MAXHOST))
                guard getnameinfo(address, socklen_t(data.count), &buffer, socklen_t(buffer.count), nil, 0, NI_NUMERICHOST) == 0 else { return nil }
                return String(cString: buffer)
            }
            if let numeric { host = numeric; break }
        }
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

private struct TVShelfDestination: Hashable { let id: String; let play: Bool }

struct TVRootView: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    @Environment(\.scenePhase) var phase
    @State private var settings = false
    @State private var section = "Home"
    @State private var path = NavigationPath()
    @State private var pendingShelfURL: URL?
    /// The focused section tab, or nil while focus is in the page below.
    @FocusState private var focusedTab: String?
    @State private var homeReset = UUID()
    /// Back from anywhere in a section returns to the top of Home; Back from the tabs on Home leaves the app.
    private var returnHome: (() -> Void)? {
        guard store.configured, !settings, path.isEmpty, section != "Home" || focusedTab == nil else { return nil }
        return {
            section = "Home"; homeReset = UUID()
            DispatchQueue.main.async { focusedTab = "Home" }
        }
    }
    private let sections = ["Home", "Movies", "TV Shows", "Torrents", "Search", "Settings"]
    var body: some View {
        NavigationStack(path: $path) {
            Group {
                if !store.configured || settings {
                    TVConnectionView { settings = false }
                } else {
                    switch section {
                    case "Movies": TVLibraryView(kind: "movie")
                    case "TV Shows": TVLibraryView(kind: "tv")
                    case "Torrents": TVTorrentView()
                    case "Search": TVSearchView()
                    case "Settings": TVSettingsView(connection: { settings = true })
                    default: TVLibraryView(kind: nil).id(homeReset)
                    }
                }
            }
            .navigationDestination(for: LibraryDestination.self) { route in TVMediaDetail(titleID: route.id) }
            .navigationDestination(for: TVShelfDestination.self) { route in TVMediaDetail(titleID: route.id, autoPlay: route.play) }
        }
        .ignoresSafeArea(.container, edges: [.top, .horizontal])
        .safeAreaInset(edge: .top, spacing: 0) {
            ZStack {
                if store.configured && path.isEmpty {
                    HStack(spacing: 8) {
                        ForEach(sections, id: \.self) { item in
                            Button { settings = false; section = item } label: {
                                Text(LocalizedStringKey(item)).font(.system(size: 24, weight: .semibold))
                            }.buttonStyle(TVMonochromeButtonStyle(selected: section == item)).focused($focusedTab, equals: item)
                        }
                    }
                    HStack(spacing: 8) {
                        Button { store.choosingProfile = true } label: {
                            if let profile = store.activeProfile { ProfileAvatar(profile: profile, size: 40) }
                            else { Image(systemName: "person.crop.circle") }
                        }.buttonStyle(TVMonochromeButtonStyle()).accessibilityLabel("Switch user")
                        Image("TorrbabyLogo").resizable().scaledToFit().frame(width: 44, height: 44)
                        Spacer()
                    }
                }
                HStack { Spacer(); TVLanguagePicker(onDown: path.isEmpty ? nil : { NotificationCenter.default.post(name: Notification.Name("TVFocusDetailPlay"), object: nil) },
                                             onExit: path.isEmpty ? returnHome : { path.removeLast(); DispatchQueue.main.async { focusedTab = section } }) }
            }
            .frame(height: 56).padding(.horizontal, 20).padding(.top, 8).padding(.bottom, 12)
            .focusSection()
            // Moving up into the bar lands on the current section's tab, wherever focus came from.
            .defaultFocus($focusedTab, section, priority: .userInitiated)
        }
        .background(Color.black.ignoresSafeArea())
        .tint(.white)
        .ignoresSafeArea(.container, edges: [.top, .horizontal])
        .onExitCommand(perform: returnHome)
        .overlay(alignment: .bottom) { ContinueWatchingUndoBanner() }
        .task(id: phase) {
            while !Task.isCancelled && phase == .active {
                await store.refresh(polling: true)
                openShelfSelection()
                await TVTopShelfPublisher.publish(store)
                try? await Task.sleep(for: .seconds(10))
            }
        }
        .onOpenURL { url in pendingShelfURL = url; openShelfSelection() }
        .onChange(of: store.activeProfile?.id) { _, _ in TVTopShelfPublisher.clear() }
        .task(id: interfaceLanguage.selection) { await TVTopShelfPublisher.publish(store) }
        .alert(ActionAlert(store.actionError, fallbackTitle: "Couldn’t complete the request").title, isPresented: Binding(get: { store.actionError != nil }, set: { if !$0 { store.actionError = nil } })) {
            Button("OK") { store.actionError = nil }
        } message: { Text(ActionAlert(store.actionError, fallbackTitle: "Couldn’t complete the request").message) }
    }
    private func openShelfSelection() {
        guard let url = pendingShelfURL, url.scheme == "torrbaby", url.host == "library",
              let query = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems,
              let id = query.first(where: { $0.name == "title" })?.value,
              store.profileReady, store.library != nil else { return }
        pendingShelfURL = nil
        guard store.library?.titles.contains(where: { $0.id == id }) == true else { return }
        let sameProfile = query.first(where: { $0.name == "profile" })?.value == store.activeProfile?.id
        let play = sameProfile && query.first(where: { $0.name == "action" })?.value == "play"
        settings = false; path = NavigationPath(); path.append(TVShelfDestination(id: id, play: play))
    }
}

/// Settings in the system's own form: the app's mark on the left, a grouped list on the right.
private struct TVSettingsView: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    let connection: () -> Void
    @State private var showSubtitles = false
    var body: some View {
        HStack(alignment: .center, spacing: 0) {
            VStack(spacing: 22) {
                Image("TorrbabyLogo").resizable().scaledToFit().frame(width: 300, height: 300)
                    .clipShape(RoundedRectangle(cornerRadius: 56))
                Text(verbatim: "Torrbaby").font(.title2.bold())
            }.frame(maxWidth: .infinity)
            List {
                SwiftUI.Section("Library") {
                    NavigationLink { TorrentSearchView() } label: { Text("Find torrents") }
                }
                SwiftUI.Section {
                    Button { showSubtitles = true } label: { Text("Default subtitles") }
                    if let status = store.status {
                        Toggle("Process library automatically", isOn: Binding(
                            get: { status.settings.backgroundSubtitles ?? true },
                            set: { enabled in store.perform { try await store.send("PATCH", "/v1/settings", body: ["background_subtitles": enabled]) } }
                        )).disabled(store.busy)
                    }
                } header: { Text("Subtitles") } footer: {
                    if let preparation = store.status?.subtitlePreparation {
                        let working = preparation.current.flatMap { job in
                            store.library?.titles.flatMap(\.assets).first { $0.id == job.assetId }?.name
                                .map { name in job.totalCues > 0 ? "\(name) · \(Int(Double(job.completedCues) / Double(job.totalCues) * 100))%" : name }
                        }
                        Text(verbatim: [L10n.format("%lld processed · %lld scanned", preparation.translated, preparation.scanned), working ?? ""]
                            .filter { !$0.isEmpty }.joined(separator: "\n"))
                    }
                }
                SwiftUI.Section("Server") {
                    LabeledContent { Text(LocalizedStringKey(store.connected ? "Connected" : "Offline")) } label: { Text("Status") }
                    Button(action: connection) { Text("Connection settings") }
                }
                SwiftUI.Section("About") {
                    LabeledContent { Text(verbatim: AppRelease.description.replacingOccurrences(of: "Version ", with: "")).monospacedDigit() } label: { Text("Version") }
                }
            }.listStyle(.grouped).frame(maxWidth: 900)
        }.padding(.horizontal, 60)
        .sheet(isPresented: $showSubtitles) { LocalizedInterface { SubtitlePreferencesView() } }
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
                    TVFeaturedHero(title: featured)
                }
                if kind == nil {
                    let continuing = titles.filter { $0.isContinuing }.sorted { $0.continueWatchingAt > $1.continueWatchingAt }
                    TVShelf(name: "Continue watching", titles: continuing)
                    TVShelf(name: "Recently added", titles: titles)
                    TVShelf(name: "Movies", titles: titles.filter { $0.kind == "movie" })
                    TVShelf(name: "TV Shows", titles: titles.filter { $0.kind == "tv" })
                    let watched = titles.filter { $0.watched }.sorted { ($0.assets.map(\.lastPlayed).max() ?? 0) > ($1.assets.map(\.lastPlayed).max() ?? 0) }
                    TVShelf(name: "Recently watched", titles: watched)
                    let genres = Array(Set(titles.flatMap(\.genres))).sorted()
                    ForEach(genres, id: \.self) { genre in
                        let matches = titles.filter { $0.genres.contains(genre) }
                        if matches.count >= 7 { TVShelf(name: genre, titles: matches) }
                    }
                } else {
                    Text(LocalizedStringKey(kind == "movie" ? "Movies" : "TV Shows")).font(.largeTitle.bold())
                    TVPosterGrid(titles: titles.sorted { $0.title.localizedStandardCompare($1.title) == .orderedAscending })
                }
                if titles.isEmpty {
                    if store.library == nil { ProgressView().tint(.white).frame(maxWidth: .infinity, minHeight: 160) }
                    else { Text("Your library is empty").font(.headline).foregroundStyle(.secondary).padding(.vertical, 40) }
                }
                if let error = store.libraryError { Text(L10n.text(error)).foregroundStyle(.orange) }
            }.padding(.horizontal, 32).padding(.top, 8).padding(.bottom, 40)
        }.background(Color.black)
    }

}

private struct TVMonochromeButtonStyle: ButtonStyle {
    var selected = false
    @Environment(\.isFocused) private var focused
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .padding(.horizontal, 18).padding(.vertical, 8)
            .foregroundStyle(focused || selected ? Color.black : .white)
            .background(focused || selected ? Color.white : .clear, in: RoundedRectangle(cornerRadius: 8))
            .scaleEffect(focused ? 1.025 : 1)
            .opacity(configuration.isPressed ? 0.75 : 1)
            .animation(.easeOut(duration: 0.15), value: focused)
    }
}

private struct TVLanguagePicker: View {
    var onDown: (() -> Void)? = nil
    /// Back while the switch is focused over a pushed page: go back a page, never out of the app.
    var onExit: (() -> Void)? = nil
    @ObservedObject private var language = InterfaceLanguage.shared
    var body: some View {
        if let onDown {
            controls.onMoveCommand { direction in if direction == .down { onDown() } }.onExitCommand(perform: onExit)
        } else { controls.onExitCommand(perform: onExit) }
    }
    private var controls: some View {
        HStack(spacing: 2) {
            Button { language.choose("en") } label: { Text(verbatim: "EN") }
                .buttonStyle(TVMonochromeButtonStyle(selected: language.code == "en"))
            Button { language.choose("zh-Hans") } label: { Text(verbatim: "中文") }
                .buttonStyle(TVMonochromeButtonStyle(selected: language.code == "zh-Hans"))
        }.font(.system(size: 22, weight: .semibold))
    }
}

private struct TVFeaturedHero: View {
    let title: LibraryTitle
    @FocusState private var focused: Bool
    var body: some View {
        ZStack(alignment: .bottomLeading) {
            LibraryArtwork(id: title.id, variant: "backdrop", alignment: .top)
            LinearGradient(colors: [.clear, .black.opacity(0.88)], startPoint: .center, endPoint: .bottom)
            VStack(alignment: .leading, spacing: 16) {
                Text(title.title).font(.system(size: 48, weight: .bold)).lineLimit(2)
                Text(([title.yearLabel] + title.genres.prefix(2)).filter { !$0.isEmpty }.joined(separator: " · "))
                    .font(.headline)
                NavigationLink(value: LibraryDestination(id: title.id)) {
                    Label("Explore", systemImage: "play.fill").font(.headline)
                }.buttonStyle(TVMonochromeButtonStyle()).focused($focused)
            }.foregroundStyle(.white).padding(44)
        }.frame(height: 650)
            .clipShape(RoundedRectangle(cornerRadius: 18))
            .overlay(RoundedRectangle(cornerRadius: 18).stroke(.white.opacity(focused ? 0.65 : 0), lineWidth: 2))
            .scaleEffect(focused ? 1.006 : 1)
            .animation(.easeOut(duration: 0.15), value: focused)
            .defaultFocus($focused, true)
            .task { focused = true }
    }
}

private struct TVMediaDetail: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    let titleID: String
    var autoPlay = false
    @State private var didAutoPlay = false
    @FocusState private var heroFocused: Bool
    @FocusState private var moreFocused: Bool
    @FocusState private var summaryFocused: Bool
    @FocusState private var episodeFocus: String?
    @FocusState private var seasonFocus: Int?
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
                        LibraryArtwork(id: title.id, variant: title.backdrop == nil ? "poster" : "backdrop", alignment: .top)
                        LinearGradient(colors: [.clear, .black.opacity(0.95)], startPoint: .top, endPoint: .bottom)
                        VStack(alignment: .leading, spacing: 22) {
                            Text(title.title).font(.system(size: 56, weight: .bold)).lineLimit(2)
                            Text(LocalizedStringKey(([title.yearLabel, title.contentRating ?? ""] + title.genres.prefix(3)).filter { !$0.isEmpty }.joined(separator: " · ")))
                                .font(.headline).foregroundStyle(.white.opacity(0.8))
                            if let asset = title.resume ?? title.playable.first(where: { !$0.watched }) ?? title.playable.first {
                                // TV shows keep the episode guide directly below the hero, so their
                                // secondary actions live in one compact menu beside Play.
                                HStack(spacing: 14) {
                                    Button { play(asset, title: title) } label: {
                                        Label(asset.playbackActionTitle(television: title.kind == "tv"), systemImage: "play.fill")
                                            .font(.headline)
                                    }.buttonStyle(TVMonochromeButtonStyle()).focused($heroFocused)
                                    if title.kind == "tv" {
                                        TVMoreActions(asset: asset, title: title) { play(asset, title: title, fromBeginning: true) }
                                            .focused($moreFocused)
                                    }
                                }.focusSection().defaultFocus($heroFocused, true, priority: .userInitiated)
                            }
                        }.padding(44).frame(maxWidth: 1400, alignment: .leading)
                    }.frame(height: 650)
                        .clipShape(RoundedRectangle(cornerRadius: 18))
                        .overlay(RoundedRectangle(cornerRadius: 18).stroke(.white.opacity(heroFocused || moreFocused ? 0.65 : 0), lineWidth: 2))
                        .scaleEffect(heroFocused || moreFocused ? 1.006 : 1)
                    VStack(alignment: .leading, spacing: 38) {
                        if !title.summary.isEmpty {
                            // A stop on the way down, so the description can be read; a show's opens in full while focused.
                            Text(title.summary).font(.system(size: 27)).lineSpacing(7)
                                .lineLimit(title.kind == "tv" && !summaryFocused ? 3 : nil)
                                .frame(maxWidth: 1300, alignment: .leading)
                                .padding(.horizontal, 18).padding(.vertical, 14)
                                .background(.white.opacity(summaryFocused ? 0.1 : 0), in: RoundedRectangle(cornerRadius: 14))
                                .padding(.horizontal, -18).padding(.vertical, -14)
                                .focusable().focusEffectDisabled().focused($summaryFocused)
                                .animation(.easeOut(duration: 0.15), value: summaryFocused)
                        }
                        if let asset = title.resume ?? title.playable.first(where: { !$0.watched }) ?? title.playable.first {
                            MediaLanguagesView(info: asset.mediaInfo)
                        }
                        if title.kind != "tv", let asset = title.resume ?? title.playable.first(where: { !$0.watched }) ?? title.playable.first {
                            VStack(alignment: .leading, spacing: 8) {
                                if let members = title.cast, !members.isEmpty {
                                    NavigationLink { LibraryCast(title: title.title, members: members, television: title.kind == "tv") } label: {
                                        Label("Cast", systemImage: "person.2")
                                    }
                                }
                                if asset.position > 0 {
                                    Button { play(asset, title: title, fromBeginning: true) } label: {
                                        Label("Start from Beginning", systemImage: "backward.end.fill")
                                    }
                                }
                                TVDetailActions(asset: asset, title: title)
                            }.buttonStyle(TVMonochromeButtonStyle()).focusSection()
                        }
                        // Extras are their own tab after the last season, not a tail on every season.
                        let extras = title.assets.filter { $0.role == "extra" }
                        let seasons = Array(Set(title.assets.filter { $0.role != "extra" }.compactMap(\.season))).sorted()
                        let tabs = title.kind == "tv" && !extras.isEmpty ? seasons + [LibraryTitle.extrasTab] : seasons
                        let chosen = season.flatMap { tabs.contains($0) ? $0 : nil }
                        let selectedSeason = chosen ?? title.resume?.season ?? title.playable.first(where: { !$0.watched && ($0.season ?? 0) > 0 })?.season ?? seasons.first(where: { $0 > 0 }) ?? seasons.first
                        // Shown even for a single season, so every show's page has the same shape.
                        if title.kind == "tv" && !tabs.isEmpty {
                            ScrollView(.horizontal) {
                                HStack(spacing: 25) {
                                    ForEach(tabs, id: \.self) { number in
                                        Button(LocalizedStringKey(number == LibraryTitle.extrasTab ? "Extras" : number == 0 ? "Specials" : "Season \(number)")) { season = number }
                                            .buttonStyle(TVMonochromeButtonStyle(selected: selectedSeason == number))
                                            .focused($seasonFocus, equals: number)
                                    }
                                }.padding(15)
                            }.scrollClipDisabled().focusSection().defaultFocus($seasonFocus, selectedSeason, priority: .userInitiated)
                        }
                        let showingExtras = title.kind == "tv" && selectedSeason == LibraryTitle.extrasTab
                        let assets = showingExtras ? extras : title.assets.filter { $0.role != "extra" && (title.kind != "tv" || $0.season == selectedSeason) }
                            .sorted { ($0.episode ?? 0, $0.name ?? "") < ($1.episode ?? 0, $1.name ?? "") }
                        if title.kind == "tv" || assets.count > 1 {
                            // Arriving from the hero or season row lands on the next episode to watch.
                            let upNext = title.resume ?? title.playable.first(where: { !$0.watched })
                            let entry = assets.first(where: { $0.id == upNext?.id && $0.complete }) ?? assets.first(where: \.complete)
                            VStack(alignment: .leading, spacing: 38) {
                                ForEach(assets) { asset in assetButton(asset, title: title).focused($episodeFocus, equals: asset.id) }
                            }.focusSection().defaultFocus($episodeFocus, entry?.id, priority: .userInitiated)
                        }
                        let unnumbered = title.assets.filter { $0.role != "extra" && $0.season == nil }
                        if title.kind == "tv" && selectedSeason != nil && !showingExtras && !unnumbered.isEmpty {
                            Text("Other episodes").font(.title2.bold())
                            ForEach(unnumbered) { asset in assetButton(asset, title: title) }
                        }
                        if title.kind != "tv" && !extras.isEmpty {
                            Text("Special features").font(.title2.bold())
                            ForEach(extras) { asset in assetButton(asset, title: title) }
                        }
                    }.padding(.horizontal, 12).padding(.bottom, 70)
                }.padding(.horizontal, 32).padding(.top, 8)
            }
        }.background(Color.black)
            .overlay(alignment: .bottom) {
                if loading {
                    HStack(spacing: 16) {
                        ProgressView().tint(.white)
                        Text(LocalizedStringKey(preparation.isEmpty ? "Preparing playback…" : preparation))
                    }.padding(20).background(.black.opacity(0.9), in: RoundedRectangle(cornerRadius: 10))
                        .padding(.bottom, 24).allowsHitTesting(false)
                }
            }
            .toolbar(.hidden, for: .navigationBar)
            .fullScreenCover(item: $playback) { PlayerScreen(playback: $0, store: store).onExitCommand { playback = nil } }
            .onReceive(NotificationCenter.default.publisher(for: Notification.Name("TVFocusDetailPlay"))) { _ in heroFocused = true }
            .defaultFocus($heroFocused, true)
            .task(id: title?.id) {
                if title != nil { heroFocused = true }
                guard autoPlay, !didAutoPlay, let title,
                      let asset = title.resume ?? title.playable.first(where: { !$0.watched }) ?? title.playable.first else { return }
                didAutoPlay = true; play(asset, title: title)
            }
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
                Image(systemName: "play.circle.fill").font(.largeTitle)
                VStack(alignment: .leading, spacing: 10) {
                    if let episode = asset.episode { Text("Episode \(episode)").font(.caption) }
                    Text(asset.name ?? title.title).font(.headline).lineLimit(2)
                    if !asset.summary.isEmpty { Text(asset.summary).font(.caption).lineLimit(2) }
                    if !asset.complete { Text("Downloading").font(.caption).foregroundStyle(Theme.muted) }
                    if asset.position > 0 && asset.duration > 0 {
                        ProgressView(value: min(1, asset.position / asset.duration)).tint(Theme.accent)
                    }
                }
                Spacer()
                if asset.duration > 0 { Text("\(Int(asset.duration / 60)) min").font(.caption) }
            }.padding(28).frame(maxWidth: .infinity, alignment: .leading)
        }.buttonStyle(.card).disabled(!asset.complete)
        }
    }
    private func play(_ asset: LibraryAsset, title: LibraryTitle, fromBeginning: Bool = false) {
        guard !loading else { return }
        loading = true; preparation = ""
        playbackTask = Task {
            defer { loading = false; playbackTask = nil }
            do {
                try store.requireStreamingConnection()
                await store.flushProgress()
                try await store.registerClient()
                let link = try await store.mediaPlaybackLink(asset: asset, titleID: title.id) { preparation = $0 }
                playback = Playback(sessionId: link.sessionId, resumePosition: fromBeginning ? 0 : (link.resumePosition ?? 0),
                    url: try store.endpoint(link.path), name: asset.episode == nil ? title.title : title.title + " · " + (asset.name ?? "Episode \(asset.episode!)"), libraryAssetID: asset.id, libraryTitleID: title.id)
            } catch { if !Task.isCancelled { store.actionError = error.localizedDescription } }
        }
    }
}

private struct TVMoreActions: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    let asset: LibraryAsset
    let title: LibraryTitle
    let restart: () -> Void
    @State private var showMenu = false
    @State private var showCast = false
    @State private var showFiles = false
    var body: some View {
        Button { showMenu = true } label: {
            Text("More…").font(.headline)
        }.buttonStyle(TVMonochromeButtonStyle())
        .confirmationDialog(title.title, isPresented: $showMenu, titleVisibility: .visible) {
            if title.cast?.isEmpty == false { Button("Cast") { showCast = true } }
            if asset.position > 0 { Button("Start from Beginning", action: restart) }
            Button(LocalizedStringKey(asset.watched ? "Mark as Unwatched" : "Mark as Watched")) {
                Task {
                    do {
                        try await store.send("POST", "/v1/library/assets/\(asset.id)/watched", body: ["watched": !asset.watched])
                        await store.refresh()
                    } catch { store.actionError = error.localizedDescription }
                }
            }
            if asset.complete { Button("Manage files") { showFiles = true } }
            if title.isContinuing {
                Button("Remove from Continue Watching") {
                    Task { await store.setContinueWatchingHidden(title.id, hidden: true) }
                }
            }
            Button("Cancel", role: .cancel) {}
        }
        // The app-wide white tint would hide the focused row's label on its white platter.
        .tint(nil)
        .navigationDestination(isPresented: $showCast) {
            LibraryCast(title: title.title, members: title.cast ?? [], television: true)
        }
        .navigationDestination(isPresented: $showFiles) { LibraryFileManagement(assetID: asset.id) }
    }
}

private struct TVDetailActions: View {
    @EnvironmentObject var store: Store
    let asset: LibraryAsset
    let title: LibraryTitle
    @State private var showFiles = false
    @State private var saving = false
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Button {
                saving = true
                Task {
                    defer { saving = false }
                    do {
                        try await store.send("POST", "/v1/library/assets/\(asset.id)/watched", body: ["watched": !asset.watched])
                        await store.refresh()
                    } catch { store.actionError = error.localizedDescription }
                }
            } label: {
                Label(LocalizedStringKey(asset.watched ? "Mark as Unwatched" : "Mark as Watched"), systemImage: asset.watched ? "circle" : "checkmark.circle")
            }
            if asset.complete {
                Button { showFiles = true } label: { Label("Manage files", systemImage: "folder") }
            }
            if title.isContinuing {
                Button("Remove from Continue Watching") {
                    Task { await store.setContinueWatchingHidden(title.id, hidden: true) }
                }
            }
        }.buttonStyle(TVMonochromeButtonStyle()).disabled(saving)
        .navigationDestination(isPresented: $showFiles) { LibraryFileManagement(assetID: asset.id) }
    }
}

private struct TVShelf: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    let name: String
    let titles: [LibraryTitle]
    @FocusState private var focusedID: String?
    /// The poster last focused here; its long-press menu takes focus away before an item is removed.
    @State private var lastFocused: String?
    var body: some View {
        if !titles.isEmpty {
            VStack(alignment: .leading, spacing: 24) {
                Text(L10n.text(name)).font(.title2.bold())
                ScrollView(.horizontal) {
                    LazyHStack(alignment: .top, spacing: 24) {
                        ForEach(titles) { TVPoster(title: $0, continuing: name == "Continue watching", focus: $focusedID).frame(width: 220) }
                    }.padding(20)
                }.scrollClipDisabled().scrollIndicators(.hidden)
            }.focusSection()
            .onChange(of: focusedID) { _, id in if let id { lastFocused = id } }
            .onChange(of: titles.map(\.id)) { old, new in
                // When the focused poster leaves the shelf, focus its right-hand neighbour, or the left one at the end.
                guard let removed = lastFocused, !new.contains(removed), let index = old.firstIndex(of: removed) else { return }
                let next = old[(index + 1)...].first(where: new.contains) ?? old[..<index].last(where: new.contains)
                lastFocused = next
                DispatchQueue.main.async { focusedID = next }
            }
        }
    }
}


private struct TVPosterButtonStyle: ButtonStyle {
    @Environment(\.isFocused) private var focused
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .overlay(RoundedRectangle(cornerRadius: 16).stroke(.white.opacity(focused ? 0.65 : 0), lineWidth: 2))
            .scaleEffect(focused ? 1.045 : 1)
            .brightness(configuration.isPressed ? -0.08 : 0)
            .animation(.easeOut(duration: 0.15), value: focused)
    }
}

private struct TVPoster: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    let title: LibraryTitle
    var continuing = false
    var focus: FocusState<String?>.Binding? = nil
    var body: some View {
        VStack(alignment: .center, spacing: 10) {
            if let focus { link.focused(focus, equals: title.id) } else { link }
            if continuing, let asset = title.resume {
                ZStack(alignment: .leading) {
                    Rectangle().fill(.white.opacity(0.22))
                    Rectangle().fill(.white).frame(width: 220 * (asset.duration > 0 ? min(1, max(0, asset.position / asset.duration)) : 0))
                }.frame(width: 220, height: 3).padding(.top, 8).accessibilityHidden(true)
                VStack(alignment: .center, spacing: 5) {
                    if let season = asset.season, let episode = asset.episode {
                        Text(L10n.format("Season %d Ep %d", season, episode))
                    }
                    if asset.duration > 0 {
                        let minutes = Int(ceil(max(0, asset.duration - asset.position) / 60))
                        Text(minutes >= 60 ? L10n.format("%dh %dm left", minutes / 60, minutes % 60) : L10n.format("%dm left", minutes))
                    }
                    if !title.yearLabel.isEmpty { Text(title.yearLabel) }
                }.font(.system(size: 21)).foregroundStyle(Theme.muted)
            } else {
                Text(title.yearLabel).font(.system(size: 21)).foregroundStyle(Theme.muted).lineLimit(1)
            }
        }.multilineTextAlignment(.center)
    }
    private var link: some View {
        NavigationLink(value: LibraryDestination(id: title.id)) {
            LibraryArtwork(id: title.id).frame(width: 220, height: 330)
                .clipShape(RoundedRectangle(cornerRadius: 16))
                .overlay { if title.poster?.isEmpty != false { Text(title.title).font(.system(size: 25, weight: .semibold)).multilineTextAlignment(.center).padding() } }
                .overlay(alignment: .topTrailing) { if title.watched { WatchedBadge() } }
        }.accessibilityLabel(title.title).buttonStyle(TVPosterButtonStyle()).contextMenu { ContinueWatchingAction(title: title) }
    }
}

private struct TVPosterGrid: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    let titles: [LibraryTitle]
    var body: some View {
        LazyVGrid(columns: [GridItem(.adaptive(minimum: 220), spacing: 24)], alignment: .leading, spacing: 40) {
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
            if category == "Needs attention" { return !$0.isJunk && $0.needsAttention }
            if $0.fileOperation != nil { return category == "Seeding" }
            return category == "Downloading" ? !$0.completed : category == "Seeding" ? $0.completed && $0.status == 6 : $0.completed && $0.status != 6
        })
    }
    var body: some View {
        VStack(spacing: 30) {
            Picker("Torrents", selection: $category) {
                ForEach(["Downloading", "Seeding", "Complete", "Needs attention"], id: \.self) { Text(LocalizedStringKey($0)).tag($0) }
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
                            if category == "Needs attention", let issue = torrent.progressIssue {
                                Text(L10n.text(issue)).font(.caption).foregroundStyle(.orange)
                            }
                            if let operation = torrent.fileOperation {
                                Text(LocalizedStringKey(operation.label)).font(.caption).foregroundStyle(operation.error == nil ? Color.secondary : Color.red)
                            }
                            HStack(spacing: 32) {
                                Text(bytes(torrent.size))
                                Label(rate(torrent.rateDownload), systemImage: "arrow.down").foregroundStyle(Theme.accent)
                                Label(rate(torrent.rateUpload), systemImage: "arrow.up").foregroundStyle(.cyan)
                                Text("Ratio \(max(0, torrent.uploadRatio), specifier: "%.2f")")
                            }.font(.caption).monospacedDigit()
                            if !torrent.completed {
                                GeometryReader { geometry in
                                    ZStack(alignment: .leading) {
                                        Color.white.opacity(0.18)
                                        Color.white.frame(width: geometry.size.width * min(1, max(0, torrent.percentDone)))
                                        Text(torrent.percentDone, format: .percent.precision(.fractionLength(0)))
                                            .font(.system(size: 20, weight: .bold)).frame(maxWidth: .infinity)
                                            .blendMode(.difference)
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
