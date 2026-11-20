#if !os(tvOS)
import SwiftUI
import Charts

enum Section: String, CaseIterable, Identifiable {
    case findTorrents = "Find", watching = "Watching", downloads = "Downloads", library = "Home", movies = "Movies", shows = "TV Shows", active = "Downloading", seeding = "Seeding", complete = "Complete", subtitles = "Processing", attention = "Needs attention", automation = "Automation", settings = "Settings"
    var id: String { rawValue }
    var icon: String {
        switch self {
        case .findTorrents: return "magnifyingglass"
        case .watching: return "eye"
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
    /// Filled symbol and colour for the iPhone menu's icon squares, as in iOS Settings.
    var badge: (symbol: String, tint: Color) {
        switch self {
        case .library: return ("play.rectangle.fill", .blue)
        case .findTorrents: return ("magnifyingglass", .indigo)
        case .watching: return ("eye.fill", .cyan)
        case .movies: return ("film.fill", .red)
        case .shows: return ("tv.fill", .purple)
        case .downloads: return ("arrow.down.circle.fill", .green)
        case .active: return ("waveform.path", .teal)
        case .seeding: return ("arrow.up.circle.fill", .mint)
        case .complete: return ("checkmark.circle.fill", .gray)
        case .subtitles: return ("captions.bubble.fill", .pink)
        case .attention: return ("exclamationmark.circle.fill", .orange)
        case .automation: return ("sparkles", .yellow)
        case .settings: return ("gearshape.fill", .gray)
        }
    }

}

struct SubtitleQueueView: View {
    @ObservedObject private var activity = ServerActivity.shared
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    @Environment(\.scenePhase) private var phase
    private let groups = [("Needs attention", ["failed", "stale", "expired"]), ("Waiting", ["queued", "scanning", "interrupted"]), ("Finished", ["ready"])]
    var body: some View { statusContent.modifier(NeedsServerStatus()) }
    @ViewBuilder private var statusContent: some View {
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
                if let queues = preparation.queues, !queues.isEmpty {
                    SwiftUI.Section { ForEach(queues) { queueRow($0) } }
                }
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
            } else {
                ProgressView { Text("Loading processing queue…").multilineTextAlignment(.center) }
                    .frame(maxWidth: .infinity, alignment: .center).padding(.vertical, 8)
            }
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
    /// One queue: files waiting, how many run now, and files finished per minute over the last hour.
    private func queueRow(_ queue: SubtitlePreparation.Queue) -> some View {
        let name = ["subtitles": "Subtitles", "translation": "Translation", "credits": "Credits"][queue.id] ?? queue.id
        return HStack(spacing: 12) {
            Text(LocalizedStringKey(name)).font(.subheadline.weight(.semibold))
            Spacer()
            Text(verbatim: L10n.format("%lld waiting", queue.waiting)).monospacedDigit()
            if queue.running > 0 { Text(verbatim: L10n.format("%lld running", queue.running)).monospacedDigit().foregroundStyle(Color.accentColor) }
            Text(verbatim: L10n.format("%@ per min", queue.perMinute.formatted(.number.precision(.fractionLength(queue.perMinute < 10 ? 1 : 0)))))
                .monospacedDigit().foregroundStyle(.secondary)
        }.font(.caption).textSelection(.enabled)
    }
    /// One file: what it is, then its steps. A file being worked on lists each step on its own
    /// line with progress; the others show their steps on one line.
    @ViewBuilder private func row(_ item: SubtitleQueueItem) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(verbatim: item.show ?? item.title).font(.subheadline.weight(.semibold)).lineLimit(1).truncationMode(.middle)
            if let line = item.episodeLine { Text(verbatim: line).font(.caption).lineLimit(1).truncationMode(.tail) }
            if item.state == "running" || item.state == "failed" {
                ForEach(item.steps) { step in
                    VStack(alignment: .leading, spacing: 2) {
                        HStack(spacing: 6) {
                            stepIcon(step.status)
                            Text(LocalizedStringKey(step.label)).lineLimit(1)
                            Spacer()
                            if let progress = step.progress { Text(verbatim: progress).monospacedDigit() }
                        }
                        if let fraction = step.fraction { ProgressView(value: fraction).controlSize(.mini) }
                    }.font(.caption).foregroundStyle(.secondary)
                }
            } else {
                HStack(spacing: 10) {
                    ForEach(item.steps) { step in
                        HStack(spacing: 3) { stepIcon(step.status); Text(LocalizedStringKey(step.label)) }
                    }
                }.font(.caption2).foregroundStyle(.secondary).lineLimit(1)
            }
            if item.subtitlesState == "not_applicable" { Text("No supported subtitle source").font(.caption).foregroundStyle(.secondary) }
            if let error = item.error { Text(L10n.text(error)).font(.caption).foregroundStyle(.orange).lineLimit(2) }
        }.padding(.vertical, 2).frame(maxWidth: .infinity, alignment: .leading)
    }
    private func stepIcon(_ status: SubtitleQueueItem.Step.Status) -> some View {
        let (name, color): (String, Color) = switch status {
        case .done: ("checkmark.circle.fill", .green)
        case .running: ("circle.dotted.circle", .accentColor)
        case .waiting: ("circle", .secondary)
        case .skipped: ("minus.circle", .secondary)
        case .failed: ("exclamationmark.circle.fill", .orange)
        }
        return Image(systemName: name).foregroundStyle(color).accessibilityLabel(Text(LocalizedStringKey(
            status == .done ? "Done" : status == .running ? "In progress" : status == .waiting ? "Waiting" : status == .skipped ? "Not needed" : "Failed")))
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

/// The connection status at the foot of a sidebar. Tapping it opens Settings → Connection.
struct SidebarConnectionStatus: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    let open: () -> Void
    var body: some View {
        Button(action: open) {
            HStack(spacing: 7) {
                Circle().fill(store.connectionColor).frame(width: 7, height: 7)
                Text(LocalizedStringKey(store.connectionLabel))
                    .font(.caption.weight(.semibold))
            }
            .frame(maxWidth: .infinity, alignment: .center)
            .padding(.horizontal, 18).padding(.vertical, 6)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help("Connection")
        .accessibilityHint(Text("Opens connection settings"))
    }
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
    // iPhone opens on Home, as every other platform does; its back button leads to the menu.
    @State private var preferredColumn: NavigationSplitViewColumn = .detail
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
    /// The home hero's Play: the title's page goes on the stack with no animation while the
    /// player covers the screen, so closing the player lands there (and Back returns home).
    private func openTitleBeneathPlayer(_ id: String) {
        var next = location
        next.path.append(LibraryDestination(id: id))
        var transaction = Transaction()
        transaction.disablesAnimations = true
        withTransaction(transaction) { navigate(next) }
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
        true
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
    private static let menuGroups: [(title: String, sections: [Section])] = [
        ("Library", [.library, .findTorrents, .movies, .shows, .watching, .downloads]),
        ("Torrents", [.active, .seeding, .complete, .subtitles, .attention]),
        ("Server", [.automation, .settings])
    ]
    /// The sidebar of the Mac and iPad.
    private var sidebarMenu: some View {
        GeometryReader { geometry in
            let rowHeight = mobileSidebar ? min(52, max(40, (geometry.size.height - 120) / 11)) : 34
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    ForEach(Self.menuGroups, id: \.title) { group in
                        menuSection(LocalizedStringKey(group.title), sections: group.sections, rowHeight: rowHeight)
                    }
                }.padding(.horizontal, 12).padding(.vertical, 6)
            }
        }
    }
    #if os(iOS)
    /// iPhone: the menu is a whole screen, so it is a standard grouped list with coloured
    /// icon squares, like iOS Settings.
    private var phoneMenu: some View {
        List {
            ForEach(Self.menuGroups, id: \.title) { group in
                SwiftUI.Section {
                    ForEach(group.sections) { section in
                        Button { navigate(Location(section: section)) } label: {
                            HStack(spacing: 12) {
                                Image(systemName: section.badge.symbol).font(.system(size: 15, weight: .semibold)).foregroundStyle(.white)
                                    .frame(width: 29, height: 29).background(section.badge.tint.gradient, in: RoundedRectangle(cornerRadius: 7))
                                    .accessibilityHidden(true)
                                Text(LocalizedStringKey(section.rawValue)).foregroundStyle(Color.primary)
                                Spacer(minLength: 0)
                                Image(systemName: "chevron.right").font(.footnote.weight(.semibold)).foregroundStyle(.tertiary).accessibilityHidden(true)
                            }.contentShape(Rectangle())
                        }.buttonStyle(.plain)
                    }
                } header: { Text(LocalizedStringKey(group.title)) }
            }
        }.listStyle(.insetGrouped).scrollContentBackground(.hidden)
    }
    #endif
    @State private var showConnect = false
    @State private var torrentDraft: TorrentDraft?
    #if DEBUG
    struct DebugScreen: Identifiable { let id: String }
    @State private var debugScreen: DebugScreen?
    #endif
    #if os(iOS)
    /// iPad: Settings takes the whole screen, its own categories becoming the sidebar, as in
    /// iOS Settings; the library sidebar is gone until Done returns to the previous page.
    private var settingsFullScreen: Bool { sizeClass == .regular && selection == .settings }
    private func leaveSettings() {
        if past.isEmpty { navigate(Location(section: .library)) } else { back() }
    }
    #endif
    @ViewBuilder private var rootContent: some View {
        #if os(iOS)
        if settingsFullScreen { PadSettingsView(showConnect: $showConnect, done: leaveSettings) } else { librarySplit }
        #else
        librarySplit
        #endif
    }
    private var librarySplit: some View {
        NavigationSplitView(preferredCompactColumn: $preferredColumn) {
            VStack(alignment: .leading, spacing: sidebarSpacing) {
                // On iPad the wordmark shares the bar row with the sidebar button instead.
                if !brandInBar { sidebarBrand.padding(.horizontal, mobileSidebar ? 20 : 24).padding(.vertical, 12) }
                #if os(iOS)
                if sizeClass == .compact { phoneMenu } else { sidebarMenu }
                #else
                sidebarMenu
                #endif
                SidebarConnectionStatus {
                    SettingsRequest.shared.page = .connection
                    navigate(Location(section: .settings))
                }
                #if os(iOS)
                // iPhone: the status sits low, beside the home indicator, leaving the list the room.
                .padding(.bottom, sizeClass == .compact ? -22 : 0)
                #endif
            }
            #if os(macOS)
            .navigationSplitViewColumnWidth(min: 220, ideal: 240, max: 280)
            #endif
            .background(Theme.sidebar.ignoresSafeArea())
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                // The wordmark sits at the leading edge of the bar; on iPhone the language
                // switch is opposite it.
                if sizeClass == .compact { LanguageToolbar() }
                if #available(iOS 26, *) {
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
                    else if selection == .watching { WatchingView() }
                    else if selection == .findTorrents { TorrentSearchView(initialQuery: UserDefaults.standard.string(forKey: "initialQuery") ?? "") }
                    else if selection == .subtitles { SubtitleQueueView() }
                    else if selection == .automation { AutomationView() }
                    else if selection == .settings { SettingsView(showConnect: $showConnect) }
                    else if selection == .library || selection == nil { LibraryBrowser(showDownloads: { navigate(Location(section: .downloads)) }, openTitleBeneathPlayer: openTitleBeneathPlayer) }
                    else if selection == .movies { LibraryBrowser(kind: "movie", showDownloads: { navigate(Location(section: .downloads)) }).id("movies") }
                    else if selection == .shows { LibraryBrowser(kind: "tv", showDownloads: { navigate(Location(section: .downloads)) }).id("shows") }
                    else { TorrentListView(section: selection ?? .active) }
                }
                .background(Theme.canvas)
                // Not on the Mac: a painted window-toolbar background covered the title bar, and
                // the window could not be dragged from anywhere along its top.
                #if !os(macOS)
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
    }
    var body: some View {
        rootContent
        #if os(macOS)
        .focusedSceneValue(\.addTorrent, { torrentDraft = TorrentDraft() })
        .focusedSceneValue(\.goBack, past.isEmpty ? nil : { back() })
        .focusedSceneValue(\.goForward, future.isEmpty ? nil : { forward() })
        .background(MouseHistoryNavigation(back: { back() }, forward: { forward() }))
        #endif
        .overlay(alignment: .bottom) { ContinueWatchingUndoBanner() }
        .sheet(isPresented: $showConnect) { LocalizedInterface { ConnectionView() } }
        #if DEBUG
        // Local screenshot checks only: TORRBABY_DEBUG_SCREEN=drives opens Settings → Library,
        // =files:<asset id> opens that file's Manage Files, =title:<id> a title, =processing the
        // Processing page. Not in Release builds.
        .sheet(item: $debugScreen) { screen in
            LocalizedInterface {
                NavigationStack {
                    if screen.id == "drives" { SettingsPageView(page: .library, showConnect: $showConnect) }
                    else if screen.id.hasPrefix("files:") { LibraryFileManagement(assetID: String(screen.id.dropFirst(6))) }
                    else if screen.id.hasPrefix("title:") { LibraryDetail(titleID: String(screen.id.dropFirst(6))) }
                    else if screen.id == "processing" { SubtitleQueueView() }
                    else if screen.id == "general" { SettingsPageView(page: .general, showConnect: $showConnect) }
                    else if screen.id == "connection" { SettingsPageView(page: .connection, showConnect: $showConnect) }
                }
            }
        }
        .task {
            if let screen = ProcessInfo.processInfo.environment["TORRBABY_DEBUG_SCREEN"], !screen.isEmpty {
                try? await Task.sleep(for: .seconds(3)); debugScreen = DebugScreen(id: screen)
            }
        }
        #if os(iOS)
        // Simulator layout checks without input: TORRBABY_DEBUG_LEAVE_SETTINGS=<seconds> presses
        // Settings' Done on iPad.
        .task {
            if let delay = ProcessInfo.processInfo.environment["TORRBABY_DEBUG_LEAVE_SETTINGS"].flatMap(Double.init) {
                try? await Task.sleep(for: .seconds(delay))
                if settingsFullScreen { leaveSettings() }
            }
        }
        #endif
        #endif
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
            NoticeBanner().padding(.bottom, 12)
        }
        .alert(ActionAlert(store.actionError, fallbackTitle: "Couldn’t complete that action").title, isPresented: Binding(get: { store.actionError != nil }, set: { if !$0 { store.actionError = nil } })) {
            Button("OK") { store.actionError = nil }
        } message: { Text(ActionAlert(store.actionError, fallbackTitle: "Couldn’t complete that action").message) }
        .task(id: phase) {
            if !store.configured { showConnect = true }
            #if !os(macOS)
            // Back in the foreground: everything at once; then the periodic refresh, which
            // checks in fully once a minute and keeps on-screen live data current.
            if phase == .active { store.refreshFullyNext() }
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
    /// Cloud-drive downloads, listed among the torrents: running ones on Downloading, finished ones on Complete.
    @State private var cloudJobs: [CloudJob] = []
    private struct CloudJobList: Decodable { let jobs: [CloudJob] }
    private var jobs: [CloudJob] {
        guard section == .active || section == .complete else { return [] }
        return cloudJobs.filter { ($0.state == "done") == (section == .complete) && (search.isEmpty || $0.title.localizedCaseInsensitiveContains(search)) }
    }
    private func loadCloudJobs() async {
        if let list: CloudJobList = try? await store.request("GET", "/v1/cloud/jobs", timeout: 15) { cloudJobs = list.jobs }
    }
    #if os(macOS)
    @FocusState private var searchFocused: Bool
    #endif
    @State private var removing: Torrent?
    @State private var processingIssues = 0
    /// Needs attention: choosing several torrents to remove at once.
    @State private var selecting = false
    @State private var selected: Set<String> = []
    @State private var confirmingBulkRemove = false
    @State private var bulkProgress: String?
    /// The header's rates as one line on iPhone and iPad, as cards on the Mac.
    #if os(iOS)
    private let compact = true
    #else
    private let compact = false
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
    var body: some View { torrentContent.modifier(NeedsLiveTorrents()) }
    @ViewBuilder private var torrentContent: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 24) {
                if !compact { VStack(alignment: .leading, spacing: 6) {
                    Text(LocalizedStringKey(section.rawValue))
                        .font(.system(size: 30, weight: .bold, design: .rounded))
                } }
                // Out of reach, the server's torrents, speeds and history are unknown: say so
                // rather than showing none, zero and an empty chart.
                if !store.connected {
                    if store.offline {
                        ContentUnavailableView("Can’t reach your server", systemImage: "wifi.slash",
                                               description: Text("Torrents and downloads appear when your server can be reached again."))
                            .padding(.vertical, 40)
                    } else {
                        ProgressView("Connecting…").frame(maxWidth: .infinity).padding(.vertical, 40)
                    }
                } else {
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
                if section == .active { MediaDriveSpace(compact: compact) }
                if section == .seeding { LibraryOperationStatus(search: search) }
                if let error = store.torrentError {
                    Label("Couldn’t refresh torrents: " + error, systemImage: "exclamationmark.triangle")
                        .font(.callout).foregroundStyle(.orange)
                }
                // Cloud downloads sit above the torrents, in the order they will run.
                if !jobs.isEmpty {
                    VStack(alignment: .leading, spacing: compact ? 4 : 10) {
                        Text(verbatim: jobs.count == 1 ? L10n.text("1 DOWNLOAD") : L10n.format("%d DOWNLOADS", jobs.count)).font(.caption.weight(.semibold)).tracking(1.6).foregroundStyle(Theme.muted)
                        ForEach(jobs) { job in CloudJobRow(job: job, changed: { Task { await loadCloudJobs() } }) }
                    }
                }
                HStack {
                    Text("\(filtered.count) \(filtered.count == 1 ? "TORRENT" : "TORRENTS")").font(.caption.weight(.semibold)).tracking(1.6).foregroundStyle(Theme.muted)
                    Spacer()
                    Picker("Sort", selection: sortSelection) {
                        ForEach(TorrentSort.choices(section.rawValue)) { Text(LocalizedStringKey($0.label)).tag($0) }
                    }.pickerStyle(.menu).fixedSize()
                }
                if section == .attention { ProcessingAttention(count: $processingIssues) }
                if section == .attention && !filtered.isEmpty { bulkBar }
                if filtered.isEmpty && (section != .seeding || store.detachedLibraryOperations.isEmpty) && !(section == .attention && processingIssues > 0) {
                    ContentUnavailableView(search.isEmpty ? "All clear" : "No matches", systemImage: search.isEmpty ? "checkmark.circle" : "magnifyingglass", description: Text(LocalizedStringKey(search.isEmpty ? "" : "Try a different title.")))
                        .padding(.vertical, 40)
                } else {
                    LazyVStack(spacing: 6) {
                        ForEach(filtered) { torrent in listItem(torrent) }
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
        .task(id: store.server) {
            guard section == .active || section == .complete else { return }
            while !Task.isCancelled {
                await loadCloudJobs()
                try? await Task.sleep(for: .seconds(cloudJobs.contains(where: \.isActive) ? 3 : 15))
            }
        }
        .removeTorrentAlert($removing)
        .confirmationDialog(L10n.format("Remove %d torrents?", selectedTorrents.count), isPresented: $confirmingBulkRemove, titleVisibility: .visible) {
            Button(L10n.format("Remove %d torrents, keep files", selectedTorrents.count)) { removeSelected(deleteData: false) }
            Button(L10n.format("Remove %d torrents and delete files", selectedTorrents.count), role: .destructive) { removeSelected(deleteData: true) }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Deleting files permanently removes their downloaded data, including partial downloads.")
        }
        .onChange(of: filtered.map(\.id)) { _, ids in selected.formIntersection(ids) }
        #if os(iOS)
        .toolbar { LanguageToolbar() }
        #endif
        .refreshable { await store.refresh() }
    }
    private func listItem(_ torrent: Torrent) -> some View {
            VStack(spacing: 6) {
              Group { if selecting {
                  Button { toggle(torrent) } label: {
                      HStack(spacing: 12) {
                          Image(systemName: selected.contains(torrent.id) ? "checkmark.circle.fill" : "circle")
                              .font(.title3).foregroundStyle(selected.contains(torrent.id) ? Theme.accent : Theme.muted)
                          row(torrent)
                      }.contentShape(Rectangle())
                  }.buttonStyle(.plain)
                  .disabled(torrent.fileOperation != nil)
                  .accessibilityAddTraits(selected.contains(torrent.id) ? .isSelected : [])
              } else {
                  NavigationLink(value: torrent.id) { row(torrent) }.buttonStyle(.plain)
              } }
              .contextMenu {
                  // The whole row is a link, so its text cannot be selected; copying is offered here.
                  Button { Clipboard.copy(torrent.displayName) } label: { Label("Copy name", systemImage: "doc.on.doc") }
                  Divider()
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
    private func row(_ torrent: Torrent) -> some View { TransferRow(torrent: torrent, showReason: section == .attention) }
    private var selectable: [Torrent] { filtered.filter { $0.fileOperation == nil } }
    private var selectedTorrents: [Torrent] { selectable.filter { selected.contains($0.id) } }
    private func toggle(_ torrent: Torrent) {
        if selected.contains(torrent.id) { selected.remove(torrent.id) } else { selected.insert(torrent.id) }
    }
    /// Select, Select all and Remove for the torrents in Needs attention.
    @ViewBuilder private var bulkBar: some View {
        HStack(spacing: 10) {
            if selecting {
                Button(LocalizedStringKey(selectedTorrents.count == selectable.count ? "Select none" : "Select all")) {
                    selected = selectedTorrents.count == selectable.count ? [] : Set(selectable.map(\.id))
                }.buttonStyle(MonoButtonStyle(compact: true)).fixedSize()
                Text(L10n.format("%d selected", selectedTorrents.count)).font(.caption.weight(.semibold)).monospacedDigit().foregroundStyle(Theme.muted)
                Spacer(minLength: 8)
                if let bulkProgress {
                    ProgressView().controlSize(.small)
                    Text(verbatim: bulkProgress).font(.caption).monospacedDigit().foregroundStyle(Theme.muted)
                } else {
                    Button(role: .destructive) { confirmingBulkRemove = true } label: { Label("Remove…", systemImage: "trash") }
                        .buttonStyle(MonoButtonStyle(compact: true)).fixedSize()
                        .disabled(selectedTorrents.isEmpty || !store.connected)
                }
                Button("Done") { selecting = false; selected = [] }.buttonStyle(MonoButtonStyle(compact: true)).fixedSize().disabled(bulkProgress != nil)
            } else {
                Spacer(minLength: 0)
                Button { selecting = true } label: { Label("Select", systemImage: "checkmark.circle") }
                    .buttonStyle(MonoButtonStyle(compact: true)).fixedSize()
            }
        }
    }
    /// Removes the selected torrents one at a time, each confirmed by its own hash as a single removal is.
    private func removeSelected(deleteData: Bool) {
        let targets = selectedTorrents
        guard !targets.isEmpty, bulkProgress == nil else { return }
        Task {
            var failures: [String] = []
            for (index, torrent) in targets.enumerated() {
                bulkProgress = L10n.format("Removing %d of %d…", index + 1, targets.count)
                do {
                    try await store.send("POST", "/v1/torrents/\(torrent.id)/remove", body: ["delete_data": deleteData, "confirm_hash": torrent.id])
                    selected.remove(torrent.id)
                } catch { failures.append(torrent.displayName + ": " + error.localizedDescription) }
            }
            bulkProgress = nil
            if failures.isEmpty { selecting = false } else {
                store.actionError = L10n.format("%d torrents could not be removed", failures.count) + "\n" + failures.joined(separator: "\n")
            }
            await store.refresh()
        }
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

/// A transfer as Find shows a release: its poster, the title the library knows it by, the
/// release's tags (picture, sound, source, seasons and episodes), then one line of state,
/// size, speed and time left, over a thin progress bar while it downloads.
struct TransferRow: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    let torrent: Torrent
    var showReason = false
    private var title: (name: String, year: String?) { torrent.title(store.metadata.first { $0.hash == torrent.id }?.selected) }
    private var stateColor: Color { torrent.needsAttention ? .orange : torrent.completed ? Theme.muted : Theme.accent }
    private var tags: [SearchTag] { (torrent.scope.map { [SearchTag(text: $0)] } ?? []) + (torrent.hints?.tags ?? []) }
    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            TorrentThumbnail(torrent: torrent).frame(width: SearchFont.poster, height: SearchFont.poster * 1.45)
            VStack(alignment: .leading, spacing: 4) {
                HStack(alignment: .firstTextBaseline, spacing: 5) {
                    Text(verbatim: title.name).font(SearchFont.rowTitle).foregroundStyle(Theme.torrent).lineLimit(2).multilineTextAlignment(.leading)
                    if let year = title.year { Text(verbatim: year).font(SearchFont.detail).foregroundStyle(Theme.muted) }
                }
                if !tags.isEmpty { SearchTags(tags: tags) }
                status.font(SearchFont.detail.monospacedDigit()).foregroundStyle(Theme.muted).lineLimit(1).minimumScaleFactor(0.85)
                if !torrent.completed { TransferBar(value: torrent.percentDone, color: stateColor) }
                if let operation = torrent.fileOperation {
                    Text(LocalizedStringKey(operation.label)).font(SearchFont.detail).foregroundStyle(operation.error == nil ? Color.secondary : Color.orange)
                        .fixedSize(horizontal: false, vertical: true)
                }
                if showReason, let reason = torrent.progressIssue { Text(reason).font(SearchFont.detail).foregroundStyle(.orange).lineLimit(2) }
            }.layoutPriority(1).frame(maxWidth: .infinity, alignment: .leading)
            Image(systemName: "chevron.right").font(.caption2.weight(.semibold)).foregroundStyle(Theme.muted).padding(.top, 3)
        }
        .padding(.vertical, 9).padding(.horizontal, 11)
        .background(Theme.panel, in: RoundedRectangle(cornerRadius: 9))
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
    }
    /// "Downloading · 42% of 8.3 GB · ↓ 24 Mbps · ~12m", or for a finished one "Seeding · 8.3 GB · ↑ 3.1 Mbps · Ratio 1.02".
    @ViewBuilder private var status: some View {
        HStack(spacing: 5) {
            Text(LocalizedStringKey(torrent.stateName)).foregroundStyle(stateColor)
            dot
            if torrent.completed { Text(verbatim: bytes(torrent.size)) }
            else { Text(verbatim: L10n.format("%@ of %@", torrent.percentDone.formatted(.percent.precision(.fractionLength(0))), bytes(torrent.size))) }
            if !torrent.completed && torrent.status == 4 {
                dot; Label(rate(torrent.rateDownload), systemImage: "arrow.down").labelStyle(TightLabel()).foregroundStyle(Theme.accent)
                dot; Text(verbatim: torrent.etaText)
            } else if torrent.rateUpload > 0 || torrent.status == 6 {
                dot; Label(rate(torrent.rateUpload), systemImage: "arrow.up").labelStyle(TightLabel())
                dot; Text(verbatim: L10n.format("Ratio %@", max(0, torrent.uploadRatio).formatted(.number.precision(.fractionLength(2)))))
            }
            if torrent.status == 4 || torrent.status == 6 { dot; Text(verbatim: peerCount(torrent.peersConnected)) }
        }
    }
    private var dot: some View { Text(verbatim: "·") }
}
/// An icon and its text close together, for figures in a line.
struct TightLabel: LabelStyle {
    func makeBody(configuration: Configuration) -> some View {
        HStack(spacing: 2) { configuration.icon.imageScale(.small); configuration.title }
    }
}
/// A thin progress bar for a row; the detail page keeps the labelled gauge.
struct TransferBar: View {
    let value: Double
    var color = Theme.accent
    private var fraction: Double { value.isFinite ? min(1, max(0, value)) : 0 }
    var body: some View {
        GeometryReader { geometry in
            ZStack(alignment: .leading) {
                Capsule().fill(color.opacity(0.16))
                Capsule().fill(color).frame(width: max(3, geometry.size.width * fraction))
            }
        }.frame(height: 3).padding(.top, 2)
            .accessibilityElement(children: .ignore).accessibilityLabel("Download progress")
            .accessibilityValue(Text(verbatim: "\(Int(fraction * 100))%"))
    }
}

func peerCount(_ count: Int) -> String { L10n.format(count == 1 ? "%lld peer" : "%lld peers", count) }

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
    /// How far the banner has been dragged; past 60 points in any direction it goes.
    @State private var drag: CGSize = .zero
    var body: some View {
        if let notice = store.notices.first {
            let dismiss = { withAnimation(.easeOut(duration: 0.2)) { store.notices.removeAll { $0.id == notice.id } }; drag = .zero }
            HStack(spacing: 12) {
                Image(systemName: "checkmark.circle.fill").foregroundStyle(Theme.accent)
                VStack(alignment: .leading, spacing: 3) {
                    Text(notice.title).font(.headline)
                    if !notice.detail.isEmpty { Text(notice.detail).font(.caption).lineLimit(2) }
                }.accessibilityElement(children: .combine)
                Spacer(minLength: 4)
                // A large target: the banner covers part of the page until it goes.
                Button(action: dismiss) {
                    Image(systemName: "xmark").font(.body.weight(.semibold))
                        .frame(width: 44, height: 44).background(Theme.canvas.opacity(0.6), in: Circle())
                        .contentShape(Circle())
                }.buttonStyle(.plain).accessibilityLabel("Dismiss notification")
            }.padding(.vertical, 10).padding(.leading, 16).padding(.trailing, 10).frame(maxWidth: 480)
                .background(Theme.panel, in: RoundedRectangle(cornerRadius: 10))
                .shadow(radius: 10).padding(.horizontal)
                .offset(drag).opacity(1 - min(0.6, Double(max(abs(drag.width), abs(drag.height))) / 200))
                .gesture(DragGesture(minimumDistance: 8)
                    .onChanged { drag = $0.translation }
                    .onEnded { value in
                        if max(abs(value.translation.width), abs(value.translation.height)) > 60 { dismiss() }
                        else { withAnimation(.spring(duration: 0.25)) { drag = .zero } }
                    })
                .task(id: notice.id) {
                    drag = .zero
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
