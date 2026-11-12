#if !os(tvOS)
import SwiftUI
import Charts

enum Section: String, CaseIterable, Identifiable {
    case findTorrents = "Find", downloads = "Downloads", library = "Home", movies = "Movies", shows = "TV Shows", active = "Downloading", seeding = "Seeding", complete = "Complete", subtitles = "Processing", attention = "Needs attention", automation = "Automation", settings = "Settings"
    var id: String { rawValue }
    var icon: String {
        switch self {
        case .findTorrents: return "magnifyingglass"
        case .downloads: return "arrow.down.circle"
        case .movies: return "film"
        case .shows: return "tv"
        case .active: return "waveform.path"
        case .subtitles: return "captions.bubble"
        case .complete: return "checkmark.circle"
        case .seeding: return "arrow.up.circle"
        case .attention: return "exclamationmark.circle"
        case .library: return "play.rectangle"
        case .automation: return "sparkles"
        case .settings: return "slider.horizontal.3"
        }
    }

}

struct SubtitleQueueView: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    @Environment(\.scenePhase) private var phase
    private let groups = [("Needs attention", ["failed", "stale", "expired"]), ("Waiting", ["queued", "scanning", "interrupted"]), ("Finished", ["ready"])]
    var body: some View {
        List {
            if let error = store.subtitleQueueError {
                Label(error, systemImage: "wifi.exclamationmark").foregroundStyle(.secondary)
            }
            if store.status?.settings.backgroundSubtitles == false {
                Label("Automatic processing paused", systemImage: "pause.circle")
            } else if store.status?.subtitlePreparation?.waitingForIdle == true {
                Label("Waiting for playback to finish", systemImage: "play.circle")
            }
            if let preparation = store.status?.subtitlePreparation {
                // Files being processed scroll with the rest; several run at once.
                let running = (preparation.queue ?? []).filter { $0.state == "running" }
                if !running.isEmpty {
                    SwiftUI.Section("\(L10n.text("Now processing")) · \(running.count)") {
                        ForEach(running) { item in row(item) }
                    }
                }
                ForEach(groups, id: \.0) { group in
                    // A file whose last attempt failed needs attention even while a retry waits.
                    let items = (preparation.queue ?? []).filter { item in
                        item.state != "running" && (group.0 == "Needs attention"
                            ? group.1.contains(item.state) || item.error != nil
                            : group.1.contains(item.state) && item.error == nil)
                    }
                    if !items.isEmpty {
                        SwiftUI.Section("\(L10n.text(group.0)) · \(items.count)") {
                            ForEach(items) { item in
                                row(item)
                                    .contentShape(Rectangle())
                                    .onTapGesture { prioritize(item) }
                                    .accessibilityAction(named: "Prioritize processing") { prioritize(item) }
                                    #if os(macOS)
                                    .help(item.state == "ready" ? "" : "Click to prioritize this title in the processing queue")
                                    #endif
                            }
                        }
                    }
                }
                if preparation.queue?.isEmpty != false { Text("No processing jobs").foregroundStyle(.secondary) }
            } else { ProgressView("Loading processing queue…") }
        }
        .canvasBackground()
        .navigationTitle(L10n.text("Processing"))
        .refreshable { await store.refreshStatus() }
        .task {
            while !Task.isCancelled {
                if phase == .active { await store.refreshStatus() }
                do { try await Task.sleep(for: .seconds(2)) } catch { return }
            }
        }
    }
    /// One compact row: the title on one line, then stage and progress, then any problem.
    @ViewBuilder private func row(_ item: SubtitleQueueItem) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(item.title).font(.subheadline.weight(.semibold)).lineLimit(1).truncationMode(.middle)
            HStack(spacing: 8) {
                Text(LocalizedStringKey(item.state == "scanning" ? "Checking languages…" : item.stageLabel)).lineLimit(1)
                Spacer()
                if item.totalCues > 0 && item.state != "ready" { Text(item.progressText).monospacedDigit() }
                else if item.state != "running" { Text(LocalizedStringKey(item.state.capitalized)) }
            }.font(.caption).foregroundStyle(.secondary)
            // Only stages that count their work show a bar; the others say what they are doing.
            if item.totalCues > 0 && item.state != "ready" {
                ProgressView(value: Double(item.completedCues), total: Double(item.totalCues)).controlSize(.mini)
            }
            if item.subtitlesState == "not_applicable" { Text("No supported subtitle source").font(.caption).foregroundStyle(.secondary) }
            if let error = item.error { Text(L10n.text(error)).font(.caption).foregroundStyle(.orange).lineLimit(2) }
        }.padding(.vertical, 2).frame(maxWidth: .infinity, alignment: .leading)
    }
    private func prioritize(_ item: SubtitleQueueItem) {
        guard item.state != "ready", item.state != "running",
              let asset = item.assetId else { return }
        store.perform {
            try await store.send("POST", "/v1/library/assets/\(asset)/subtitles/prioritize")
            store.notify("Title prioritized", detail: "Starts after the current subtitle job.")
            await store.refreshStatus()
        }
    }
}

private struct TorrentDraft: Identifiable {
    let id = UUID()
    var magnet = ""
}

struct RootView: View {
    #if os(iOS)
    @Environment(\.horizontalSizeClass) private var sizeClass
    #endif
    private var pageOwnsLanguageControl: Bool {
        #if os(iOS)
        // Torrent pages add their own view switch, then the language control after it.
        return [Section.active, .seeding, .complete, .attention].contains(selection ?? .library)
        #else
        return [Section.library, .movies, .shows, .active, .seeding, .complete, .attention].contains(selection ?? .library)
        #endif
    }
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    #if os(macOS)
    @Environment(\.openWindow) private var openWindow
    #endif
    @Environment(\.scenePhase) var phase
    private struct Location: Equatable {
        var section: Section? = .library
        var path = NavigationPath()
    }
    // UI smoke tests can open a page directly: -initialSection "TV Shows" [-initialTitle <id>].
    private static var smokeTestStart: Location? {
        guard let name = UserDefaults.standard.string(forKey: "initialSection"), let section = Section(rawValue: name) else { return nil }
        var start = Location(section: section)
        if let id = UserDefaults.standard.string(forKey: "initialTitle") { start.path.append(LibraryDestination(id: id)) }
        return start
    }
    @State private var location = smokeTestStart ?? Location()
    @State private var preferredColumn: NavigationSplitViewColumn = smokeTestStart == nil ? .sidebar : .detail
    @State private var past: [Location] = []
    @State private var future: [Location] = []
    private var selection: Section? { location.section }
    private func navigate(_ next: Location) {
        preferredColumn = .detail
        guard next != location else { return }
        past.append(location)
        if past.count > 100 { past.removeFirst() }
        future.removeAll()
        location = next
    }
    private func back() {
        guard let previous = past.popLast() else { return }
        future.append(location)
        location = previous
    }
    private func forward() {
        guard let next = future.popLast() else { return }
        past.append(location)
        location = next
    }
    @ToolbarContentBuilder private var languageControls: some ToolbarContent { LanguageToolbar() }
    // On iOS the page's own actions sit just inside the language switch, which stays last.
    private var pageActionPlacement: ToolbarItemPlacement {
        #if os(iOS)
        .topBarTrailing
        #else
        .primaryAction
        #endif
    }
    private var ownsHistoryButtons: Bool {
        #if os(iOS)
        sizeClass == .regular
        #else
        true
        #endif
    }
    @ViewBuilder private var historyButtons: some View {
        Button(action: back) { Image(systemName: "chevron.left") }
            .help("Back").disabled(past.isEmpty)
        Button(action: forward) { Image(systemName: "chevron.right") }
            .help("Forward").disabled(future.isEmpty)
    }
    /// Back/Forward lead the bar. A phone already has its own back button and no room to
    /// spare, so the history pair is left out there and the language switch stays top right.
    @ToolbarContentBuilder private var historyControls: some ToolbarContent {
        #if os(macOS)
        ToolbarItemGroup(placement: .navigation) {
            historyButtons
            Button { MacLibraryTabs.create { openWindow(id: "library-tab") } } label: {
                Image(systemName: "plus")
            }.help("New Tab").accessibilityLabel("New Tab")
        }
        #else
        if sizeClass == .regular {
            ToolbarItemGroup(placement: .topBarLeading) { historyButtons }
        }
        #endif
    }
    private var mobileSidebar: Bool {
        #if os(iOS)
        true
        #else
        false
        #endif
    }
    private var sidebarSpacing: CGFloat {
        #if os(iOS)
        0
        #else
        6
        #endif
    }
    private var sidebarBrand: some View {
        HStack(spacing: 8) {
            Image("TorrbabyLogo").resizable().frame(width: 28, height: 28)
                .clipShape(RoundedRectangle(cornerRadius: 6)).accessibilityHidden(true)
            Text(verbatim: "torrbaby").font(.system(size: 18, weight: .bold, design: .rounded))
                .lineLimit(1).minimumScaleFactor(0.8)
        }
        .frame(maxWidth: .infinity, alignment: mobileSidebar && !brandInBar ? .center : .leading)
    }
    private var brandInBar: Bool {
        #if os(iOS)
        sizeClass == .regular
        #else
        false
        #endif
    }
    /// The same menu on every platform: the current page is a white row with black text.
    private func menuSection(_ title: LocalizedStringKey, sections: [Section], rowHeight: CGFloat) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title).font(.caption.weight(.semibold)).foregroundStyle(Theme.muted).padding(.horizontal, 12).padding(.bottom, 2)
            ForEach(sections) { section in
                let selected = location.section == section
                Button { navigate(Location(section: section)) } label: {
                    HStack(spacing: 12) {
                        Image(systemName: section.icon).frame(width: 24)
                        Text(LocalizedStringKey(section.rawValue))
                        Spacer(minLength: 0)
                    }.font(.body.weight(selected ? .semibold : .regular)).padding(.horizontal, 12).frame(height: rowHeight)
                        .foregroundStyle(selected ? Theme.onAccent : Color.white)
                        .background(selected ? Theme.accent : .clear, in: RoundedRectangle(cornerRadius: Theme.radius))
                        .contentShape(Rectangle())
                }.buttonStyle(.plain)
                    .accessibilityAddTraits(selected ? .isSelected : [])
            }
        }
    }
    @State private var showConnect = false
    @State private var torrentDraft: TorrentDraft?
    var body: some View {
        NavigationSplitView(preferredCompactColumn: $preferredColumn) {
            VStack(alignment: .leading, spacing: sidebarSpacing) {
                // On iPad the wordmark shares the bar row with the sidebar button instead.
                if !brandInBar { sidebarBrand.padding(.horizontal, mobileSidebar ? 20 : 24).padding(.vertical, 12) }
                GeometryReader { geometry in
                    let rowHeight = mobileSidebar ? min(52, max(40, (geometry.size.height - 120) / 11)) : 34
                    ScrollView {
                        VStack(alignment: .leading, spacing: 14) {
                            menuSection("Library", sections: [.library, .findTorrents, .movies, .shows, .downloads], rowHeight: rowHeight)
                            menuSection("Torrents", sections: [.active, .seeding, .complete, .subtitles, .attention], rowHeight: rowHeight)
                            menuSection("Server", sections: [.automation, .settings], rowHeight: rowHeight)
                        }.padding(.horizontal, 12).padding(.vertical, 6)
                    }
                }
                HStack(spacing: 7) {
                    Circle().fill(store.connectionColor).frame(width: 7, height: 7)
                    Text(LocalizedStringKey(store.connectionLabel))
                        .font(.caption.weight(.semibold))
                }
                .frame(maxWidth: .infinity, alignment: .center)
                .padding(.horizontal, 18).padding(.vertical, 6)
            }
            #if os(macOS)
            .navigationSplitViewColumnWidth(min: 220, ideal: 240, max: 280)
            #endif
            .background(Theme.sidebar.ignoresSafeArea())
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                if sizeClass == .compact { LanguageToolbar() }
                else if #available(iOS 26, *) {
                    ToolbarItem(placement: .topBarLeading) { sidebarBrand.fixedSize() }.sharedBackgroundVisibility(.hidden)
                } else {
                    ToolbarItem(placement: .topBarLeading) { sidebarBrand.fixedSize() }
                }
            }
            #endif
        } detail: {
            NavigationStack(path: Binding(get: { location.path }, set: { navigate(Location(section: selection, path: $0)) })) {
                Group {
                    if selection == .downloads { DownloadsView() }
                    else if selection == .findTorrents { TorrentSearchView(initialQuery: UserDefaults.standard.string(forKey: "initialQuery") ?? "") }
                    else if selection == .subtitles { SubtitleQueueView() }
                    else if selection == .automation { AutomationView() }
                    else if selection == .settings { SettingsView(showConnect: $showConnect) }
                    else if selection == .library || selection == nil { LibraryBrowser(showDownloads: { navigate(Location(section: .downloads)) }) }
                    else if selection == .movies { LibraryBrowser(kind: "movie", showDownloads: { navigate(Location(section: .downloads)) }).id("movies") }
                    else if selection == .shows { LibraryBrowser(kind: "tv", showDownloads: { navigate(Location(section: .downloads)) }).id("shows") }
                    else { TorrentListView(section: selection ?? .active) }
                }
                .background(Theme.canvas)
                #if os(macOS)
                .toolbarBackground(Theme.canvas, for: .windowToolbar)
                #else
                .toolbarBackground(Theme.canvas, for: .navigationBar)
                #endif
                .navigationTitle(L10n.text(selection?.rawValue ?? "Torrbaby"))
                #if os(iOS)
                .navigationBarTitleDisplayMode(.inline)
                #endif
                .toolbar {
                    historyControls
                    ToolbarItemGroup(placement: pageActionPlacement) {
                        if store.busy { ProgressView().controlSize(.small) }
                        if [.active, .seeding, .complete, .attention].contains(selection) {
                            Button { torrentDraft = TorrentDraft() } label: { Label("Add torrent", systemImage: "plus") }
                                .disabled(!store.connected)
                            Button { Task { await store.refresh() } } label: { Image(systemName: "arrow.clockwise") }
                                .help("Refresh").disabled(!store.configured)
                        }
                    }
                    if !pageOwnsLanguageControl { languageControls }
                }
                .navigationDestination(for: String.self) { hash in TorrentDetail(hash: hash).navigationBarBackButtonHidden(ownsHistoryButtons).toolbar { historyControls; languageControls } }
                .navigationDestination(for: LibraryDestination.self) { route in LibraryDetail(titleID: route.id).navigationBarBackButtonHidden(ownsHistoryButtons).toolbar { historyControls; languageControls } }
                .safeAreaInset(edge: .top) {
                    if store.offline {
                        HStack {
                            Label("Offline mode", systemImage: "wifi.slash").font(.subheadline.weight(.semibold))
                            Spacer()
                            if selection != .downloads {
                                Button("Downloads") { navigate(Location(section: .downloads)) }.font(.subheadline.weight(.semibold))
                            }
                            Button { Task { await store.refresh() } } label: { Image(systemName: "arrow.clockwise") }.accessibilityLabel("Try reconnecting")
                        }.padding(12).background(Color.orange.opacity(0.12)).background(Theme.canvas)  // opaque: content scrolls beneath it
                    }
                }
            }
        }
        #if os(macOS)
        .focusedSceneValue(\.addTorrent, { torrentDraft = TorrentDraft() })
        .focusedSceneValue(\.goBack, past.isEmpty ? nil : { back() })
        .focusedSceneValue(\.goForward, future.isEmpty ? nil : { forward() })
        .background(MouseHistoryNavigation(back: { back() }, forward: { forward() }))
        #endif
        .overlay(alignment: .bottom) { ContinueWatchingUndoBanner() }
        .sheet(isPresented: $showConnect) { LocalizedInterface { ConnectionView() } }
        .sheet(item: $torrentDraft) { draft in
             LocalizedInterface { AddTorrentView(initialMagnet: draft.magnet).id(draft.id) } }
        .onOpenURL { url in
            if url.scheme?.lowercased() == "magnet" {
                torrentDraft = TorrentDraft(magnet: canonicalMagnet(url.absoluteString))
            } else if url.isFileURL && url.pathExtension.lowercased() == "torrbaby" {
                // A download shared over AirDrop: add it to Downloads and show it there.
                do {
                    let title = try DownloadQueue.shared.importShared(url)
                    store.notify("Added to Downloads", detail: title, torrentHash: nil)
                    navigate(Location(section: .downloads))
                } catch { store.actionError = error.localizedDescription }
            }
        }
        .overlay(alignment: .bottom) {
            NoticeBanner { hash in
                var next = Location(section: selection)
                if let torrent = store.torrents.first(where: { $0.id == hash }) {
                    next.section = !torrent.completed ? .active : torrent.status == 6 ? .seeding : .complete
                }
                next.path.append(hash)
                navigate(next)
            }.padding(.bottom, 12)
        }
        .alert(ActionAlert(store.actionError, fallbackTitle: "Couldn’t complete that action").title, isPresented: Binding(get: { store.actionError != nil }, set: { if !$0 { store.actionError = nil } })) {
            Button("OK") { store.actionError = nil }
        } message: { Text(ActionAlert(store.actionError, fallbackTitle: "Couldn’t complete that action").message) }
        .task(id: phase) {
            if !store.configured { showConnect = true }
            #if !os(macOS)
            while !Task.isCancelled {
                if phase == .active { await store.refresh(polling: true) }
                try? await Task.sleep(for: .seconds(store.connectionError == nil ? 5 : 15))
            }
            #endif
        }
    }
}

struct TorrentListView: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    let section: Section
    @AppStorage("torrentSort.downloading") private var downloadingSort = TorrentSort.progress
    @AppStorage("torrentSort.seeding") private var seedingSort = TorrentSort.ratio
    @AppStorage("torrentSort.complete") private var completeSort = TorrentSort.alphabetic
    private var sortSelection: Binding<TorrentSort> {
        switch section.rawValue {
        case "Seeding": $seedingSort
        case "Complete": $completeSort
        default: $downloadingSort
        }
    }
    @State private var search = ""
    #if os(macOS)
    @FocusState private var searchFocused: Bool
    #endif
    @State private var removing: Torrent?
    @State private var processingIssues = 0
    #if os(iOS)
    @AppStorage("compactMobileTorrentView") private var compact = true
    #else
    @AppStorage("compactTorrentView") private var compact = false
    #endif
    var filtered: [Torrent] {
        sortSelection.wrappedValue.ordered(store.torrents.filter { t in
            let included: Bool
            switch section {
            case .active: included = t.fileOperation == nil && !t.completed
            case .seeding: included = t.fileOperation != nil || (t.completed && t.status == 6)
            case .complete: included = t.fileOperation == nil && t.completed && t.status != 6
            case .attention: included = !t.isJunk && t.needsAttention
            case .library: included = !t.isJunk && t.files.contains { $0.isVideo && $0.complete }
            default: included = true
            }
            return included && (search.isEmpty || t.name.localizedCaseInsensitiveContains(search))
        })
    }
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 24) {
                if !compact { VStack(alignment: .leading, spacing: 6) {
                    Text(LocalizedStringKey(section.rawValue))
                        .font(.system(size: 30, weight: .bold, design: .rounded))
                } }
                if !compact { ViewThatFits(in: .horizontal) {
                    HStack(spacing: 12) { metrics }
                    VStack(spacing: 12) { metrics }
                } } else {
                    HStack(spacing: 20) {
                        Label(rate(store.torrents.reduce(0) { $0 + $1.rateDownload }), systemImage: "arrow.down").foregroundStyle(Theme.accent)
                        Label(rate(store.torrents.reduce(0) { $0 + $1.rateUpload }), systemImage: "arrow.up").foregroundStyle(Theme.muted)
                    }.font(.caption.monospacedDigit())
                }
                TransferHistoryView(compact: compact)
                if section == .seeding { LibraryOperationStatus(search: search) }
                if let error = store.torrentError {
                    Label("Couldn’t refresh torrents: " + error, systemImage: "exclamationmark.triangle")
                        .font(.callout).foregroundStyle(.orange)
                }
                HStack {
                    Text("\(filtered.count) \(filtered.count == 1 ? "TORRENT" : "TORRENTS")").font(.caption.weight(.semibold)).tracking(1.6).foregroundStyle(Theme.muted)
                    Spacer()
                    Picker("Sort", selection: sortSelection) {
                        ForEach(TorrentSort.choices(section.rawValue)) { Text(LocalizedStringKey($0.label)).tag($0) }
                    }.pickerStyle(.menu).fixedSize()
                }
                if section == .attention { ProcessingAttention(count: $processingIssues) }
                if filtered.isEmpty && (section != .seeding || store.detachedLibraryOperations.isEmpty) && !(section == .attention && processingIssues > 0) {
                    ContentUnavailableView(search.isEmpty ? "All clear" : "No matches", systemImage: search.isEmpty ? "checkmark.circle" : "magnifyingglass", description: Text(LocalizedStringKey(search.isEmpty ? "" : "Try a different title.")))
                        .padding(.vertical, 40)
                } else {
                    LazyVStack(spacing: compact ? 4 : 10) {
                        ForEach(filtered) { torrent in
                          VStack(spacing: 6) {
                            NavigationLink(value: torrent.id) {
                                #if os(iOS)
                                MobileTorrentRow(torrent: torrent, showReason: section == .attention, compact: compact)
                                #else
                                if compact { CompactTorrentRow(torrent: torrent, showReason: section == .attention) }
                                else { TorrentRow(torrent: torrent, showReason: section == .attention) }
                                #endif
                            }.buttonStyle(.plain)
                            .contextMenu {
                                Button { store.action(torrent, "start") } label: {
                                    Label("Start", systemImage: "play.fill")
                                }.disabled(store.busy || !store.connected || torrent.status != 0 || torrent.fileOperation != nil)
                                Button { store.action(torrent, "stop") } label: {
                                    Label("Stop", systemImage: "stop.fill")
                                }.disabled(store.busy || !store.connected || torrent.status == 0 || torrent.fileOperation != nil)
                                // Jumps the queue: starts now even when every download slot is taken.
                                if !torrent.completed {
                                    Button { store.action(torrent, torrent.downloadsFirst ? "unprioritize" : "prioritize") } label: {
                                        Label(LocalizedStringKey(torrent.downloadsFirst ? "Stop downloading first" : "Download first"), systemImage: torrent.downloadsFirst ? "arrow.up.to.line.circle.fill" : "arrow.up.to.line")
                                    }.disabled(store.busy || !store.connected || torrent.fileOperation != nil)
                                }
                                Divider()
                                Button(role: .destructive) { removing = torrent } label: {
                                    Label("Remove…", systemImage: "trash")
                                }.disabled(store.busy || !store.connected || torrent.fileOperation != nil)
                            }
                            #if os(macOS)
                            if !compact && (!torrent.completed || torrent.status == 6) {
                                TransferHistoryView(compact: true, torrentHash: torrent.id, uploadOnly: torrent.completed)
                            }
                            #endif
                          }
                        }
                    }
                }
            }.padding(24).frame(maxWidth: 1200).frame(maxWidth: .infinity)
        }
        #if os(macOS)
        .toolbar {
            MacSearchToolbar(text: $search, focus: $searchFocused, prompt: "Find a torrent")
        }
        .focusedSceneValue(\.searchContent, { searchFocused = true })
        #elseif os(iOS)
        .modifier(MobileLibrarySearch(text: $search, prompt: "Find a torrent"))
        #else
        .searchable(text: $search, prompt: "Find a torrent")
        #endif
        .sheet(item: $removing) { item in LocalizedInterface { RemoveTorrentView(torrent: item) } }
        .toolbar {
            ToolbarItem(placement: {
                #if os(iOS)
                .topBarTrailing
                #else
                .primaryAction
                #endif
            }()) {
                Button { compact.toggle() } label: {
                    Label(LocalizedStringKey(compact ? "Comfortable view" : "Compact view"), systemImage: compact ? "rectangle.grid.1x2" : "list.bullet")
                }.help(compact ? "Switch to comfortable view" : "Switch to compact view")
            }
            #if os(iOS)
            LanguageToolbar()
            #endif
        }
        .refreshable { await store.refresh() }
    }
    @ViewBuilder var metrics: some View {
        Metric(title: "DOWNLOADING", value: rate(store.torrents.reduce(0) { $0 + $1.rateDownload }), symbol: "arrow.down", color: Theme.accent)
        Metric(title: "UPLOADING", value: rate(store.torrents.reduce(0) { $0 + $1.rateUpload }), symbol: "arrow.up", color: Theme.muted)
        Metric(title: "LIBRARY", value: "\(store.torrents.filter(\.completed).count) complete", symbol: "checkmark", color: Theme.muted)
    }
}
struct Metric: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    let title: String
    let value: String
    let symbol: String
    let color: Color
    var body: some View {
        Card {
            HStack(alignment: .top) {
                VStack(alignment: .leading, spacing: 10) {
                    Text(LocalizedStringKey(title)).font(.system(size: 10, weight: .semibold)).tracking(1.5).foregroundStyle(Theme.muted)
                    Text(value).font(.system(size: 21, weight: .semibold, design: .rounded)).monospacedDigit().lineLimit(1)
                }
                Spacer(minLength: 15)
                Image(systemName: symbol).font(.system(size: 13, weight: .bold)).foregroundStyle(color)
                    .frame(width: 28, height: 28).background(color.opacity(0.12), in: Circle())
            }
        }
    }
}

struct TorrentRow: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    let torrent: Torrent
    var showReason = false
    var color: Color { torrent.needsAttention ? .orange : torrent.completed ? Theme.muted : Theme.accent }
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
          HStack(alignment: .top, spacing: 16) {
            TorrentThumbnail(torrent: torrent).frame(width: 46, height: 64)
            VStack(alignment: .leading, spacing: 9) {
                Text(torrent.displayName).font(.system(size: 15, weight: .semibold)).lineLimit(2).multilineTextAlignment(.leading)
                HStack(spacing: 8) {
                    Text(LocalizedStringKey(torrent.stateName)).foregroundStyle(color)
                    Text("·").foregroundStyle(Theme.muted)
                    Text(bytes(torrent.size)).foregroundStyle(Theme.muted)
                    Spacer(minLength: 0)
                }.font(.caption)
                Text("\(bytes(torrent.downloaded)) of \(bytes(torrent.size))")
                    .font(.caption.monospacedDigit()).foregroundStyle(Theme.muted)
                ViewThatFits(in: .horizontal) {
                    HStack(spacing: 16) { TorrentActivity(torrent: torrent) }
                    VStack(alignment: .leading, spacing: 5) { TorrentActivity(torrent: torrent) }
                }.font(.caption).foregroundStyle(Theme.muted).monospacedDigit()
                if showReason, let reason = torrent.progressIssue { Text(reason).font(.caption).foregroundStyle(.orange) }
            }
            Image(systemName: "chevron.right").font(.caption.weight(.medium)).foregroundStyle(Theme.muted).padding(.top, 18)
          }
          if !torrent.completed { TorrentProgressGauge(value: torrent.percentDone) }
        }
        .padding(17).background(Theme.panel, in: RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).stroke(.white.opacity(0.04)))
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
    }
}

struct CompactTorrentRow: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    let torrent: Torrent
    let showReason: Bool
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
          HStack(spacing: 12) {
            TorrentThumbnail(torrent: torrent).frame(width: 32, height: 46)
            VStack(alignment: .leading, spacing: 4) {
                Text(torrent.displayName).font(.system(size: 13, weight: .medium)).lineLimit(1).frame(maxWidth: .infinity, alignment: .leading)
                HStack(spacing: 6) {
                    Text(LocalizedStringKey(torrent.stateName)).foregroundStyle(torrent.needsAttention ? .orange : torrent.completed ? Theme.muted : Theme.accent)
                        .frame(width: 76, alignment: .leading)
                    Text("\(bytes(torrent.downloaded)) / \(bytes(torrent.size))")
                        .monospacedDigit().frame(width: 150, alignment: .trailing)
                    Text(peerCount(torrent.peersConnected)).monospacedDigit()
                        .frame(width: 65, alignment: .trailing)
                }.font(.system(size: 10)).foregroundStyle(Theme.muted).lineLimit(1)
                HStack(spacing: 10) {
                    if !torrent.completed { TransferRate(value: torrent.rateDownload, upload: false) }
                    TransferRate(value: torrent.rateUpload, upload: true)
                    Text("Ratio \(max(0, torrent.uploadRatio), specifier: "%.2f")")
                        .frame(width: 82, alignment: .trailing).foregroundStyle(Theme.muted)
                    if !torrent.completed { Text(LocalizedStringKey(torrent.etaText)).frame(width: 160, alignment: .leading) }
                }.font(.caption2.monospacedDigit()).foregroundStyle(Theme.muted)
                if let operation = torrent.fileOperation {
                    Text(LocalizedStringKey(operation.label)).font(.caption2).foregroundStyle(operation.error == nil ? Color.secondary : Color.orange)
                        .fixedSize(horizontal: false, vertical: true)
                }
                if showReason, let reason = torrent.progressIssue { Text(reason).font(.caption2).foregroundStyle(.orange).lineLimit(2) }
            }
            Spacer(minLength: 0)
            Image(systemName: "chevron.right").font(.system(size: 10, weight: .medium)).foregroundStyle(Theme.muted)
          }
          if !torrent.completed { TorrentProgressGauge(value: torrent.percentDone) }
        }.padding(.horizontal, 13).padding(.vertical, 10)
            .background(Theme.panel, in: RoundedRectangle(cornerRadius: 9))
            .accessibilityElement(children: .combine)
    }
}

#if os(iOS)
/// Two deliberate layouts: compact columns on iPad, a short card on narrow screens.
/// Never turn every activity field into a separate line.
private struct MobileTorrentRow: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    let torrent: Torrent
    let showReason: Bool
    let compact: Bool
    private var color: Color { torrent.needsAttention ? .orange : torrent.completed ? Theme.muted : Theme.accent }
    var body: some View {
        ViewThatFits(in: .horizontal) {
            CompactTorrentRow(torrent: torrent, showReason: showReason).frame(minWidth: 650)
            narrow
        }
    }
    private var narrow: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .top, spacing: 12) {
                TorrentThumbnail(torrent: torrent).frame(width: 38, height: 54)
                VStack(alignment: .leading, spacing: 6) {
                    Text(torrent.displayName).font(.system(size: 14, weight: .semibold))
                        .lineLimit(compact ? 2 : 3).multilineTextAlignment(.leading)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    HStack {
                        Text(LocalizedStringKey(torrent.stateName)).foregroundStyle(color)
                        Spacer(minLength: 8)
                        Label("\(torrent.peersConnected)", systemImage: "person.2")
                            .foregroundStyle(Theme.muted)
                            .accessibilityLabel(peerCount(torrent.peersConnected))
                    }.font(.caption2).monospacedDigit()
                }
            }
            if !torrent.completed { TorrentProgressGauge(value: torrent.percentDone) }
            HStack(spacing: 10) {
                if !torrent.completed { TransferRate(value: torrent.rateDownload, upload: false, numberWidth: 65) }
                TransferRate(value: torrent.rateUpload, upload: true, numberWidth: 65)
                Spacer(minLength: 0)
                Text("Ratio \(max(0, torrent.uploadRatio), specifier: "%.2f")")
                    .foregroundStyle(Theme.muted).frame(width: 65, alignment: .trailing)
            }.font(.caption2.monospacedDigit()).lineLimit(1)
            HStack(alignment: .firstTextBaseline, spacing: 12) {
                Text(LocalizedStringKey(torrent.completed ? bytes(torrent.size) : "\(bytes(torrent.downloaded)) / \(bytes(torrent.size))"))
                    .monospacedDigit()
                Spacer(minLength: 0)
                if !torrent.completed {
                    Text(LocalizedStringKey(torrent.etaText)).monospacedDigit().multilineTextAlignment(.trailing)
                }
            }.font(.caption2).foregroundStyle(Theme.muted).lineLimit(1).minimumScaleFactor(0.85)
            if showReason, let reason = torrent.progressIssue {
                Text(reason).font(.caption2).foregroundStyle(.orange).lineLimit(2)
            }
        }.padding(14).background(Theme.panel, in: RoundedRectangle(cornerRadius: 12))
            .contentShape(Rectangle()).accessibilityElement(children: .combine)
    }
}
#endif

func peerCount(_ count: Int) -> String { L10n.format(count == 1 ? "%lld peer" : "%lld peers", count) }

private struct TransferRate: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    let value: Int64
    let upload: Bool
    var numberWidth: CGFloat = 82
    var body: some View {
        HStack(spacing: 5) {
            Image(systemName: upload ? "arrow.up" : "arrow.down").frame(width: 12)
            Text(LocalizedStringKey(value == 0 ? "0 KB/s" : rate(value)))
                .monospacedDigit().frame(width: numberWidth, alignment: .trailing)
        }.foregroundStyle(upload ? Theme.muted : Theme.accent)
            .fixedSize(horizontal: true, vertical: false)
    }
}

private struct TorrentThumbnail: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    let torrent: Torrent
    private var thumbnail: String? { store.metadata.first { $0.hash == torrent.id }?.selected?.thumbnail }
    var body: some View {
        ZStack {
            Theme.accent.opacity(0.10)
            if let thumbnail {
                LibraryArtwork(id: "torrent:" + torrent.id).id(thumbnail)
            } else {
                Image(systemName: torrent.completed ? "film" : "arrow.down")
                    .foregroundStyle(torrent.needsAttention ? .orange : Theme.accent)
            }
        }.clipShape(RoundedRectangle(cornerRadius: 5))
            .accessibilityHidden(true)
    }
}

struct TorrentProgressGauge: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    let value: Double
    var color = Theme.accent
    private var fraction: Double { value.isFinite ? min(1, max(0, value)) : 0 }
    private var label: String { "\(Int(fraction * 100))%" }
    var body: some View {
        GeometryReader { geometry in
            ZStack(alignment: .leading) {
                color.opacity(0.16)
                color.frame(width: geometry.size.width * fraction)
                Text(label).foregroundStyle(.white).frame(maxWidth: .infinity)
                Text(label).foregroundStyle(Theme.canvas).frame(maxWidth: .infinity)
                    .mask(alignment: .leading) { Rectangle().frame(width: geometry.size.width * fraction) }
            }.font(.system(size: 10, weight: .bold).monospacedDigit())
                .clipShape(RoundedRectangle(cornerRadius: 5))
        }.frame(height: 22)
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("Download progress")
            .accessibilityValue(label)
    }
}

struct TorrentActivity: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    let torrent: Torrent
    var body: some View {
        if !torrent.completed {
            TransferRate(value: torrent.rateDownload, upload: false)
        }
        TransferRate(value: torrent.rateUpload, upload: true)
        Text("\(peerCount(torrent.peersConnected)) · \(torrent.peersSendingToUs) sending").frame(width: 160, alignment: .leading)
        if !torrent.completed { Text(LocalizedStringKey(torrent.etaText)) }
        Text("Ratio \(max(0, torrent.uploadRatio), specifier: "%.2f")")
        if let uploaded = torrent.uploadedEver { Text("\(bytes(uploaded)) uploaded") }
        if torrent.activityDate > 0 {
            Text("Active \(Date(timeIntervalSince1970: torrent.activityDate), style: .relative) ago")
        }
    }
}

struct TransferHistoryView: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    let compact: Bool
    var torrentHash: String? = nil
    var uploadOnly = false
    @State private var torrentSamples: [TransferSample]?
    @AppStorage("transferHistoryHours") private var hours = 24
    var samples: [TransferSample] {
        let cutoff = Date().timeIntervalSince1970 - Double(hours * 3600)
        return (torrentHash == nil ? store.transferHistory : (torrentSamples ?? [])).filter { $0.time >= cutoff }
    }
    var body: some View {
      Group {
       if torrentHash == nil || torrentSamples != nil {
        VStack(alignment: .leading, spacing: compact ? 6 : 12) {
            HStack {
                Text(LocalizedStringKey(uploadOnly ? "UPLOAD HISTORY" : "TRANSFER HISTORY")).font(.caption2.weight(.semibold)).tracking(1)
                Spacer()
                Picker("Time range", selection: $hours) {
                    Text("1h").tag(1)
                    Text("6h").tag(6)
                    Text("24h").tag(24)
                }.pickerStyle(.segmented).frame(width: 150, height: 24)
            }.foregroundStyle(Theme.muted)
            Chart(samples) { sample in
                if !uploadOnly {
                    LineMark(x: .value("Time", sample.date), y: .value("Bytes/sec", sample.download))
                        .foregroundStyle(by: .value("Transfer", "Download"))
                }
                LineMark(x: .value("Time", sample.date), y: .value("Bytes/sec", sample.upload))
                    .foregroundStyle(by: .value("Transfer", "Upload"))
            }
            .chartForegroundStyleScale(["Download": Theme.accent, "Upload": Theme.muted])
            .chartXScale(domain: Date().addingTimeInterval(-Double(hours * 3600))...Date())
            .chartYScale(domain: .automatic(includesZero: true))
            .chartYAxis {
                AxisMarks(position: .leading, values: .automatic(desiredCount: compact ? 2 : 4)) { value in
                    AxisGridLine().foregroundStyle(.white.opacity(0.08))
                    AxisValueLabel { if let speed = value.as(Double.self) { Text(rate(Int64(speed))).font(.system(size: 9)) } }
                }
            }
            .chartXAxis { AxisMarks(values: .automatic(desiredCount: 4)) }
            .chartLegend(position: .bottom, alignment: .leading)
            .frame(height: compact ? 100 : 170)
            if samples.count < 2 {
                Text("Collecting history…").font(.caption2).foregroundStyle(Theme.muted)
            }
        }.padding(compact ? 12 : 18)
            .background(Theme.panel, in: RoundedRectangle(cornerRadius: 10))
       }
      }.task(id: store.server + (torrentHash ?? "")) {
          guard let hash = torrentHash else { return }
          while !Task.isCancelled {
              do { torrentSamples = try await store.request("GET", "/v1/torrents/\(hash)/rates") }
              catch { return }
              try? await Task.sleep(for: .seconds(30))
          }
      }
    }
}

struct NoticeBanner: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    var showTorrent: (String) -> Void
    var body: some View {
        if let notice = store.notices.first {
            HStack(spacing: 12) {
                Image(systemName: "checkmark.circle.fill").foregroundStyle(Theme.accent)
                VStack(alignment: .leading, spacing: 3) {
                    Text(notice.title).font(.headline)
                    if !notice.detail.isEmpty { Text(notice.detail).font(.caption).lineLimit(2) }
                }
                Spacer(minLength: 4)
                if let hash = notice.torrentHash {
                    Button("Show torrent") {
                        showTorrent(hash)
                        store.notices.removeAll { $0.id == notice.id }
                    }.buttonStyle(MonoButtonStyle())
                }
                Button { store.notices.removeAll { $0.id == notice.id } } label: { Image(systemName: "xmark") }
                    .buttonStyle(.plain).accessibilityLabel("Dismiss notification")
            }.padding(16).frame(maxWidth: 480)
                .background(Theme.panel, in: RoundedRectangle(cornerRadius: 10))
                .shadow(radius: 10).padding(.horizontal)
                .accessibilityElement(children: .combine)
                .task(id: notice.id) {
                    do { try await Task.sleep(for: .seconds(6)) } catch { return }
                    store.notices.removeAll { $0.id == notice.id }
                }
        }
    }
}

#endif

#if os(macOS)
/// Local event handling stays scoped to this window and never captures another app's mouse input.
private struct MouseHistoryNavigation: NSViewRepresentable {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    let back: () -> Void
    let forward: () -> Void
    final class HistoryView: NSView {
        var back: () -> Void = {}
        var forward: () -> Void = {}
        var monitor: Any?
        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            if let monitor { NSEvent.removeMonitor(monitor); self.monitor = nil }
            guard window != nil else { return }
            monitor = NSEvent.addLocalMonitorForEvents(matching: .otherMouseUp) { [weak self] event in
                guard let self, let window = self.window, window.isKeyWindow,
                      event.window === window, window.attachedSheet == nil else { return event }
                switch event.buttonNumber {
                case 3: self.back(); return nil
                case 4: self.forward(); return nil
                default: return event
                }
            }
        }
        deinit { if let monitor { NSEvent.removeMonitor(monitor) } }
    }
    func makeNSView(context: Context) -> HistoryView { HistoryView() }
    func updateNSView(_ view: HistoryView, context: Context) {
        view.back = back
        view.forward = forward
    }
}
#endif
