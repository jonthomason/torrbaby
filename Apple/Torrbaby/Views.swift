#if !os(tvOS)
import SwiftUI
import Charts

enum Section: String, CaseIterable, Identifiable {
    case downloads = "Downloads", library = "Home", movies = "Movies", shows = "TV Shows", active = "Downloading", seeding = "Seeding", complete = "Complete", subtitles = "Subtitles", attention = "Not progressing", automation = "Automation", settings = "Settings"
    var id: String { rawValue }
    var icon: String {
        switch self {
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
    @EnvironmentObject var store: Store
    private let groups = [("Working", ["running"]), ("Waiting", ["queued", "scanning", "interrupted"]), ("Finished", ["ready"]), ("Needs attention", ["failed", "stale", "expired"])]
    var body: some View {
        List {
            if store.status?.settings.backgroundSubtitles == false {
                Label("Automatic subtitles paused", systemImage: "pause.circle")
            } else if store.status?.subtitlePreparation?.waitingForIdle == true {
                Label("Waiting for playback to finish", systemImage: "play.circle")
            }
            if let preparation = store.status?.subtitlePreparation {
                ForEach(groups, id: \.0) { group in
                    let items = (preparation.queue ?? []).filter { group.1.contains($0.state) }
                    if !items.isEmpty {
                        SwiftUI.Section("\(group.0) · \(items.count)") {
                            ForEach(items) { item in
                                VStack(alignment: .leading, spacing: 8) {
                                    Text(item.title).font(.headline).lineLimit(2)
                                    HStack {
                                        Label(item.target == nil ? "Extracting" : (item.target == "en" ? "English" : "简体中文"), systemImage: "captions.bubble")
                                        Spacer()
                                        Text(item.state == "scanning" ? "Checking languages" : item.state.capitalized)
                                    }.font(.caption).foregroundStyle(.secondary)
                                    if item.totalCues > 0 && item.state != "ready" {
                                        ProgressView(value: Double(item.completedCues), total: Double(item.totalCues))
                                        Text("\(item.completedCues) / \(item.totalCues)").font(.caption.monospacedDigit()).foregroundStyle(.secondary)
                                    }
                                    if let error = item.error { Text(error).font(.caption).foregroundStyle(.orange) }
                                }.padding(.vertical, 6)
                            }
                        }
                    }
                }
                if preparation.queue?.isEmpty != false { Text("No subtitle jobs").foregroundStyle(.secondary) }
            } else { ProgressView("Loading subtitles…") }
        }
        .navigationTitle("Subtitles")
        .refreshable { await store.refresh() }
    }
}

private struct TorrentDraft: Identifiable {
    let id = UUID()
    var magnet = ""
}

struct RootView: View {
    @EnvironmentObject var store: Store
    @Environment(\.scenePhase) var phase
    private struct Location: Equatable {
        var section: Section? = .library
        var path = NavigationPath()
    }
    @State private var location = Location()
    @State private var past: [Location] = []
    @State private var future: [Location] = []
    private var selection: Section? { location.section }
    private func navigate(_ next: Location) {
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
    @ToolbarContentBuilder private var historyControls: some ToolbarContent {
        ToolbarItemGroup(placement: .navigation) {
            Button(action: back) { Image(systemName: "chevron.left") }
                .help("Back").disabled(past.isEmpty)
            Button(action: forward) { Image(systemName: "chevron.right") }
                .help("Forward").disabled(future.isEmpty)
        }
    }
    @State private var showConnect = false
    @State private var torrentDraft: TorrentDraft?
    var body: some View {
        NavigationSplitView {
            VStack(alignment: .leading, spacing: 22) {
                HStack(spacing: 11) {
                    Image("TorrbabyLogo").resizable().frame(width: 42, height: 42)
                        .clipShape(RoundedRectangle(cornerRadius: 9)).accessibilityHidden(true)
                    VStack(alignment: .leading, spacing: 1) {
                        Text("torrbaby").font(.system(size: 24, weight: .bold, design: .rounded))
                    }
                }.padding(.horizontal, 18).padding(.top, 24)
                List(selection: Binding(get: { location.section }, set: { navigate(Location(section: $0)) })) {
                    SwiftUI.Section("Library") {
                        ForEach([Section.library, .movies, .shows, .downloads]) { section in
                            Label(section.rawValue, systemImage: section.icon).padding(.vertical, 5).tag(section)
                        }
                    }
                    SwiftUI.Section("Torrents") {
                        ForEach([Section.active, .seeding, .complete, .subtitles, .attention]) { section in
                            Label(section.rawValue, systemImage: section.icon).padding(.vertical, 5).tag(section)
                        }
                    }
                    SwiftUI.Section("Server") {
                        ForEach([Section.automation, .settings]) { section in
                            Label(section.rawValue, systemImage: section.icon).padding(.vertical, 5).tag(section)
                        }
                    }
                }.listStyle(.sidebar)
                VStack(alignment: .leading, spacing: 9) {
                    HStack(spacing: 7) {
                        Circle().fill(store.connected ? Theme.mint : .orange).frame(width: 6, height: 6)
                        Text(store.connected ? "Connected" : (store.offline ? "Offline mode" : "Connecting…")).font(.caption.weight(.semibold))
                    }
                    if store.status?.settings.paused == true {
                        Label("Automation paused", systemImage: "pause.circle").font(.caption2).foregroundStyle(.orange)
                    }
                    Button("Connection settings") { showConnect = true }.font(.caption)
                }.padding(20)
            }.navigationSplitViewColumnWidth(min: 220, ideal: 240, max: 280)
        } detail: {
            NavigationStack(path: Binding(get: { location.path }, set: { navigate(Location(section: selection, path: $0)) })) {
                Group {
                    if selection == .downloads { DownloadsView() }
                    else if selection == .subtitles { SubtitleQueueView() }
                    else if selection == .automation { AutomationView() }
                    else if selection == .settings { SettingsView(showConnect: $showConnect) }
                    else if selection == .library || selection == nil { LibraryBrowser(showDownloads: { navigate(Location(section: .downloads)) }) }
                    else if selection == .movies { LibraryBrowser(kind: "movie", showDownloads: { navigate(Location(section: .downloads)) }).id("movies") }
                    else if selection == .shows { LibraryBrowser(kind: "tv", showDownloads: { navigate(Location(section: .downloads)) }).id("shows") }
                    else { TorrentListView(section: selection ?? .active) }
                }
                .background(Theme.canvas)
                .navigationTitle(selection?.rawValue ?? "Torrbaby")
                .toolbar {
                    historyControls
                    ToolbarItemGroup(placement: .primaryAction) {
                        if store.busy { ProgressView().controlSize(.small) }
                        Button { Task { await store.refresh() } } label: { Image(systemName: "arrow.clockwise") }
                            .help("Refresh").disabled(!store.configured)
                        Button { torrentDraft = TorrentDraft() } label: { Image(systemName: "plus") }.help("Add torrent")
                            .disabled(!store.connected)
                    }
                }
                .navigationDestination(for: String.self) { hash in TorrentDetail(hash: hash).navigationBarBackButtonHidden().toolbar { historyControls } }
                .navigationDestination(for: LibraryDestination.self) { route in LibraryDetail(titleID: route.id).navigationBarBackButtonHidden().toolbar { historyControls } }
                .safeAreaInset(edge: .top) {
                    if store.offline {
                        HStack {
                            Label("Offline mode", systemImage: "wifi.slash").font(.subheadline.weight(.semibold))
                            Spacer()
                            if selection != .downloads {
                                Button("Downloads") { navigate(Location(section: .downloads)) }.font(.subheadline.weight(.semibold))
                            }
                            Button { Task { await store.refresh() } } label: { Image(systemName: "arrow.clockwise") }.accessibilityLabel("Try reconnecting")
                        }.padding(12).background(.orange.opacity(0.12))
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
        .sheet(isPresented: $showConnect) { ConnectionView() }
        .sheet(item: $torrentDraft) { draft in
            AddTorrentView(initialMagnet: draft.magnet).id(draft.id)
        }
        .onOpenURL { url in
            if url.scheme?.lowercased() == "magnet" {
                torrentDraft = TorrentDraft(magnet: canonicalMagnet(url.absoluteString))
            }
        }
        .overlay(alignment: .top) {
            NoticeBanner { hash in
                var next = Location(section: selection)
                if let torrent = store.torrents.first(where: { $0.id == hash }) {
                    next.section = !torrent.completed ? .active : torrent.status == 6 ? .seeding : .complete
                }
                next.path.append(hash)
                navigate(next)
            }.padding(.top, 12)
        }
        .alert("Couldn’t complete that action", isPresented: Binding(get: { store.actionError != nil }, set: { if !$0 { store.actionError = nil } })) {
            Button("OK") { store.actionError = nil }
        } message: { Text(store.actionError ?? "") }
        .task(id: phase) {
            if !store.configured { showConnect = true }
            #if !os(macOS)
            while !Task.isCancelled {
                if phase == .active { await store.refresh() }
                try? await Task.sleep(for: .seconds(store.connectionError == nil ? 5 : 15))
            }
            #endif
        }
    }
}

struct TorrentListView: View {
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
    #if os(iOS)
    @AppStorage("compactMobileTorrentView") private var compact = true
    #else
    @AppStorage("compactTorrentView") private var compact = false
    #endif
    var filtered: [Torrent] {
        sortSelection.wrappedValue.ordered(store.torrents.filter { t in
            let included: Bool
            switch section {
            case .active: included = !t.completed
            case .seeding: included = t.completed && t.status == 6
            case .complete: included = t.completed && t.status != 6
            case .attention: included = t.needsAttention
            case .library: included = t.files.contains { $0.isVideo && $0.complete }
            default: included = true
            }
            return included && (search.isEmpty || t.name.localizedCaseInsensitiveContains(search))
        })
    }
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 24) {
                if !compact { VStack(alignment: .leading, spacing: 6) {
                    Text(section.rawValue)
                        .font(.system(size: 30, weight: .bold, design: .rounded))
                } }
                if !compact { ViewThatFits(in: .horizontal) {
                    HStack(spacing: 12) { metrics }
                    VStack(spacing: 12) { metrics }
                } } else {
                    HStack(spacing: 20) {
                        Label(rate(store.torrents.reduce(0) { $0 + $1.rateDownload }), systemImage: "arrow.down").foregroundStyle(Theme.mint)
                        Label(rate(store.torrents.reduce(0) { $0 + $1.rateUpload }), systemImage: "arrow.up").foregroundStyle(.cyan)
                    }.font(.caption.monospacedDigit())
                }
                TransferHistoryView(compact: compact)
                HStack {
                    Text("\(filtered.count) \(filtered.count == 1 ? "TORRENT" : "TORRENTS")").font(.caption.weight(.semibold)).tracking(1.6).foregroundStyle(Theme.muted)
                    Spacer()
                    Picker("Sort", selection: sortSelection) {
                        ForEach(TorrentSort.choices(section.rawValue)) { Text($0.label).tag($0) }
                    }.pickerStyle(.menu).fixedSize()
                }
                if filtered.isEmpty {
                    ContentUnavailableView(search.isEmpty ? "All clear" : "No matches", systemImage: search.isEmpty ? "checkmark.circle" : "magnifyingglass", description: Text(search.isEmpty ? "" : "Try a different title."))
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
                                }.disabled(store.busy || !store.connected || torrent.status != 0)
                                Button { store.action(torrent, "stop") } label: {
                                    Label("Stop", systemImage: "stop.fill")
                                }.disabled(store.busy || !store.connected || torrent.status == 0)
                                Divider()
                                Button(role: .destructive) { removing = torrent } label: {
                                    Label("Remove…", systemImage: "trash")
                                }.disabled(store.busy || !store.connected)
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
        .searchable(text: $search, prompt: "Find a torrent")
        #if os(macOS)
        .searchFocused($searchFocused)
        .focusedSceneValue(\.searchContent, { searchFocused = true })
        #endif
        .sheet(item: $removing) { RemoveTorrentView(torrent: $0) }
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button { compact.toggle() } label: {
                    Label(compact ? "Comfortable view" : "Compact view", systemImage: compact ? "rectangle.grid.1x2" : "list.bullet")
                }.help(compact ? "Switch to comfortable view" : "Switch to compact view")
            }
        }
        .refreshable { await store.refresh() }
    }
    @ViewBuilder var metrics: some View {
        Metric(title: "DOWNLOADING", value: rate(store.torrents.reduce(0) { $0 + $1.rateDownload }), symbol: "arrow.down", color: Theme.mint)
        Metric(title: "UPLOADING", value: rate(store.torrents.reduce(0) { $0 + $1.rateUpload }), symbol: "arrow.up", color: .cyan)
        Metric(title: "LIBRARY", value: "\(store.torrents.filter(\.completed).count) complete", symbol: "checkmark", color: .purple)
    }
}
struct Metric: View {
    let title: String
    let value: String
    let symbol: String
    let color: Color
    var body: some View {
        Card {
            HStack(alignment: .top) {
                VStack(alignment: .leading, spacing: 10) {
                    Text(title).font(.system(size: 10, weight: .semibold)).tracking(1.5).foregroundStyle(Theme.muted)
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
    let torrent: Torrent
    var showReason = false
    var color: Color { torrent.needsAttention ? .orange : torrent.completed ? .cyan : Theme.mint }
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
          HStack(alignment: .top, spacing: 16) {
            TorrentThumbnail(torrent: torrent).frame(width: 46, height: 64)
            VStack(alignment: .leading, spacing: 9) {
                Text(torrent.displayName).font(.system(size: 15, weight: .semibold)).lineLimit(2).multilineTextAlignment(.leading)
                HStack(spacing: 8) {
                    Text(torrent.stateName).foregroundStyle(color)
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
        .padding(17).background(Theme.panel, in: RoundedRectangle(cornerRadius: 16))
        .overlay(RoundedRectangle(cornerRadius: 16).stroke(.white.opacity(0.04)))
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
    }
}

struct CompactTorrentRow: View {
    let torrent: Torrent
    let showReason: Bool
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
          HStack(spacing: 12) {
            TorrentThumbnail(torrent: torrent).frame(width: 32, height: 46)
            VStack(alignment: .leading, spacing: 4) {
                Text(torrent.displayName).font(.system(size: 13, weight: .medium)).lineLimit(1).frame(maxWidth: .infinity, alignment: .leading)
                HStack(spacing: 6) {
                    Text(torrent.stateName).foregroundStyle(torrent.needsAttention ? .orange : torrent.completed ? .cyan : Theme.mint)
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
                        .frame(width: 82, alignment: .trailing).foregroundStyle(.purple.opacity(0.9))
                    if !torrent.completed { Text(torrent.etaText).frame(width: 160, alignment: .leading) }
                }.font(.caption2.monospacedDigit()).foregroundStyle(Theme.muted)
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
    let torrent: Torrent
    let showReason: Bool
    let compact: Bool
    private var color: Color { torrent.needsAttention ? .orange : torrent.completed ? .cyan : Theme.mint }
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
                        Text(torrent.stateName).foregroundStyle(color)
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
                    .foregroundStyle(.purple).frame(width: 65, alignment: .trailing)
            }.font(.caption2.monospacedDigit()).lineLimit(1)
            HStack(alignment: .firstTextBaseline, spacing: 12) {
                Text(torrent.completed ? bytes(torrent.size) : "\(bytes(torrent.downloaded)) / \(bytes(torrent.size))")
                    .monospacedDigit()
                Spacer(minLength: 0)
                if !torrent.completed {
                    Text(torrent.etaText).monospacedDigit().multilineTextAlignment(.trailing)
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

private func peerCount(_ count: Int) -> String { "\(count) " + (count == 1 ? "peer" : "peers") }

private struct TransferRate: View {
    let value: Int64
    let upload: Bool
    var numberWidth: CGFloat = 82
    var body: some View {
        HStack(spacing: 5) {
            Image(systemName: upload ? "arrow.up" : "arrow.down").frame(width: 12)
            Text(value == 0 ? "0 KB/s" : rate(value))
                .monospacedDigit().frame(width: numberWidth, alignment: .trailing)
        }.foregroundStyle(upload ? Color.cyan : Theme.mint)
            .fixedSize(horizontal: true, vertical: false)
    }
}

private struct TorrentThumbnail: View {
    @EnvironmentObject var store: Store
    let torrent: Torrent
    private var thumbnail: String? { store.metadata.first { $0.hash == torrent.id }?.selected?.thumbnail }
    var body: some View {
        ZStack {
            Theme.mint.opacity(0.10)
            if let thumbnail {
                LibraryArtwork(id: "torrent:" + torrent.id).id(thumbnail)
            } else {
                Image(systemName: torrent.completed ? "film" : "arrow.down")
                    .foregroundStyle(torrent.needsAttention ? .orange : Theme.mint)
            }
        }.clipShape(RoundedRectangle(cornerRadius: 5))
            .accessibilityHidden(true)
    }
}

struct TorrentProgressGauge: View {
    let value: Double
    var color = Color(red: 0.70, green: 0.43, blue: 1.0)
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
    let torrent: Torrent
    var body: some View {
        if !torrent.completed {
            TransferRate(value: torrent.rateDownload, upload: false)
        }
        TransferRate(value: torrent.rateUpload, upload: true)
        Text("\(peerCount(torrent.peersConnected)) · \(torrent.peersSendingToUs) sending").frame(width: 160, alignment: .leading)
        if !torrent.completed { Text(torrent.etaText) }
        Text("Ratio \(max(0, torrent.uploadRatio), specifier: "%.2f")")
        if let uploaded = torrent.uploadedEver { Text("\(bytes(uploaded)) uploaded") }
        if torrent.activityDate > 0 {
            Text("Active \(Date(timeIntervalSince1970: torrent.activityDate), style: .relative) ago")
        }
    }
}

struct TransferHistoryView: View {
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
                Text(uploadOnly ? "UPLOAD HISTORY" : "TRANSFER HISTORY").font(.caption2.weight(.semibold)).tracking(1)
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
            .chartForegroundStyleScale(["Download": Theme.mint, "Upload": Color.cyan])
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
            .background(Theme.panel, in: RoundedRectangle(cornerRadius: 14))
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
    @EnvironmentObject var store: Store
    var showTorrent: (String) -> Void
    var body: some View {
        if let notice = store.notices.first {
            HStack(spacing: 12) {
                Image(systemName: "checkmark.circle.fill").foregroundStyle(Theme.mint)
                VStack(alignment: .leading, spacing: 3) {
                    Text(notice.title).font(.headline)
                    if !notice.detail.isEmpty { Text(notice.detail).font(.caption).lineLimit(2) }
                }
                Spacer(minLength: 4)
                if let hash = notice.torrentHash {
                    Button("Show torrent") {
                        showTorrent(hash)
                        store.notices.removeAll { $0.id == notice.id }
                    }.buttonStyle(.bordered)
                }
                Button { store.notices.removeAll { $0.id == notice.id } } label: { Image(systemName: "xmark") }
                    .buttonStyle(.plain).accessibilityLabel("Dismiss notification")
            }.padding(16).frame(maxWidth: 480)
                .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 16))
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
