#if !os(tvOS)
import SwiftUI
import Charts

enum Section: String, CaseIterable, Identifiable {
    case library = "Home", movies = "Movies", shows = "TV Shows", active = "Downloading", seeding = "Seeding", complete = "Complete", attention = "Not progressing", automation = "Automation", settings = "Settings"
    var id: String { rawValue }
    var icon: String {
        switch self {
        case .movies: return "film"
        case .shows: return "tv"
        case .active: return "waveform.path"
        case .complete: return "checkmark.circle"
        case .seeding: return "arrow.up.circle"
        case .attention: return "exclamationmark.circle"
        case .library: return "play.rectangle"
        case .automation: return "sparkles"
        case .settings: return "slider.horizontal.3"
        }
    }
    var subtitle: String {
        switch self {
        case .movies: return "Movie night starts here."
        case .shows: return "Your shows, season by season."
        case .active: return "Incomplete downloads, including paused and stalled torrents."
        case .complete: return "Finished downloads that are not currently seeding."
        case .seeding: return "Downloaded. Organized. Giving back."
        case .attention: return "Stalled downloads, idle peers and errors worth a look."
        case .library: return "Your files, straight from your server."
        case .automation: return "Quietly keeping your library in order."
        case .settings: return "Make yourself at home."
        }
    }
}

struct RootView: View {
    @EnvironmentObject var store: Store
    @Environment(\.scenePhase) var phase
    @State private var selection: Section? = .library
    @State private var showConnect = false
    @State private var showAdd = false
    @State private var incomingMagnet = ""
    var body: some View {
        NavigationSplitView {
            VStack(alignment: .leading, spacing: 22) {
                HStack(spacing: 11) {
                    Image("TorrbabyLogo").resizable().frame(width: 42, height: 42)
                        .clipShape(RoundedRectangle(cornerRadius: 9)).accessibilityHidden(true)
                    VStack(alignment: .leading, spacing: 1) {
                        Text("torrbaby").font(.system(size: 24, weight: .bold, design: .rounded))
                        Text("Your home cinema").font(.caption2).foregroundStyle(Theme.muted)
                    }
                }.padding(.horizontal, 18).padding(.top, 24)
                List(selection: $selection) {
                    SwiftUI.Section("Library") {
                        ForEach([Section.library, .movies, .shows]) { section in
                            Label(section.rawValue, systemImage: section.icon).padding(.vertical, 5).tag(section)
                        }
                    }
                    SwiftUI.Section("Torrents") {
                        ForEach([Section.active, .seeding, .complete, .attention]) { section in
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
                        Text(store.connected ? "Connected" : "Not connected").font(.caption.weight(.semibold))
                    }
                    if let status = store.status {
                        Text(status.settings.paused ? "Automation is paused" : "Sorting every \(Int(status.settings.pollSeconds)) seconds")
                            .font(.caption2).foregroundStyle(Theme.muted)
                    }
                    Button("Connection settings") { showConnect = true }.font(.caption)
                }.padding(20)
            }.navigationSplitViewColumnWidth(min: 220, ideal: 240, max: 280)
        } detail: {
            NavigationStack {
                Group {
                    if selection == .automation { AutomationView() }
                    else if selection == .settings { SettingsView(showConnect: $showConnect) }
                    else if selection == .library || selection == nil { LibraryBrowser() }
                    else if selection == .movies { LibraryBrowser(kind: "movie").id("movies") }
                    else if selection == .shows { LibraryBrowser(kind: "tv").id("shows") }
                    else { TorrentListView(section: selection ?? .active) }
                }
                .background(Theme.canvas)
                .navigationTitle(selection?.rawValue ?? "Torrbaby")
                .toolbar {
                    ToolbarItemGroup(placement: .primaryAction) {
                        if store.busy { ProgressView().controlSize(.small) }
                        Button { Task { await store.refresh() } } label: { Image(systemName: "arrow.clockwise") }
                            .help("Refresh").disabled(!store.configured)
                        Button { incomingMagnet = ""; showAdd = true } label: { Image(systemName: "plus") }.help("Add torrent")
                            .disabled(!store.connected)
                    }
                }
                .navigationDestination(for: String.self) { hash in TorrentDetail(hash: hash) }
                .safeAreaInset(edge: .top) {
                    if let error = store.connectionError {
                        HStack {
                            Image(systemName: "wifi.exclamationmark")
                            Text("Connection lost · " + error).font(.caption).lineLimit(2)
                            Spacer()
                            Button("Reconnect") { showConnect = true }.font(.caption.weight(.semibold))
                        }.padding(12).background(.orange.opacity(0.12))
                    }
                }
            }
        }
        .sheet(isPresented: $showConnect) { ConnectionView() }
        .sheet(isPresented: $showAdd) { AddTorrentView(initialMagnet: incomingMagnet) }
        .onOpenURL { url in
            if url.scheme?.lowercased() == "magnet" {
                incomingMagnet = url.absoluteString
                showAdd = true
            }
        }
        .alert("Couldn’t complete that action", isPresented: Binding(get: { store.actionError != nil }, set: { if !$0 { store.actionError = nil } })) {
            Button("OK") { store.actionError = nil }
        } message: { Text(store.actionError ?? "") }
        .task(id: phase) {
            if !store.configured { showConnect = true }
            while !Task.isCancelled {
                if phase == .active { await store.refresh() }
                try? await Task.sleep(for: .seconds(store.connectionError == nil ? 5 : 15))
            }
        }
    }
}

struct TorrentListView: View {
    @EnvironmentObject var store: Store
    let section: Section
    @State private var search = ""
    @State private var removing: Torrent?
    @AppStorage("compactTorrentView") private var compact = false
    var filtered: [Torrent] {
        store.torrents.filter { t in
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
        }
    }
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 24) {
                if !compact { VStack(alignment: .leading, spacing: 6) {
                    Text(section.rawValue)
                        .font(.system(size: 30, weight: .bold, design: .rounded))
                    Text(section.subtitle).foregroundStyle(Theme.muted)
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
                    if let date = store.lastUpdated { Text(date, style: .time).font(.caption).foregroundStyle(Theme.muted) }
                }
                if filtered.isEmpty {
                    ContentUnavailableView(search.isEmpty ? "All clear" : "No matches", systemImage: search.isEmpty ? "checkmark.circle" : "magnifyingglass", description: Text(search.isEmpty ? "Torrents in this category will appear here." : "Try a different title."))
                        .padding(.vertical, 40)
                } else {
                    LazyVStack(spacing: compact ? 4 : 10) {
                        ForEach(filtered) { torrent in
                          VStack(spacing: 6) {
                            NavigationLink(value: torrent.id) {
                                if compact { CompactTorrentRow(torrent: torrent, showReason: section == .attention) }
                                else { TorrentRow(torrent: torrent, showReason: section == .attention) }
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
                            if !compact && (!torrent.completed || torrent.status == 6) {
                                TransferHistoryView(compact: true, torrentHash: torrent.id, uploadOnly: torrent.completed)
                            }
                          }
                        }
                    }
                }
            }.padding(24).frame(maxWidth: 1200).frame(maxWidth: .infinity)
        }
        .searchable(text: $search, prompt: "Find a torrent")
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
        HStack(alignment: .top, spacing: 16) {
            Image(systemName: torrent.completed ? "film.stack" : "arrow.down")
                .font(.system(size: 20, weight: .medium)).foregroundStyle(color)
                .frame(width: 46, height: 52).background(color.opacity(0.09), in: RoundedRectangle(cornerRadius: 12))
            VStack(alignment: .leading, spacing: 9) {
                Text(torrent.displayName).font(.system(size: 15, weight: .semibold)).lineLimit(2).multilineTextAlignment(.leading)
                HStack(spacing: 8) {
                    Text(torrent.stateName).foregroundStyle(color)
                    Text("·").foregroundStyle(Theme.muted)
                    Text(bytes(torrent.size)).foregroundStyle(Theme.muted)
                    Spacer(minLength: 0)
                    if !torrent.completed { Text(torrent.percentDone, format: .percent.precision(.fractionLength(0))).foregroundStyle(Theme.muted) }
                }.font(.caption)
                ProgressView(value: torrent.percentDone).tint(color)
                Text("\(bytes(torrent.downloaded)) of \(bytes(torrent.size)) · \(Int(torrent.percentDone * 100))%")
                    .font(.caption.monospacedDigit()).foregroundStyle(Theme.muted)
                ViewThatFits(in: .horizontal) {
                    HStack(spacing: 16) { TorrentActivity(torrent: torrent) }
                    VStack(alignment: .leading, spacing: 5) { TorrentActivity(torrent: torrent) }
                }.font(.caption).foregroundStyle(Theme.muted).monospacedDigit()
                if showReason, let reason = torrent.progressIssue { Text(reason).font(.caption).foregroundStyle(.orange) }
            }
            Image(systemName: "chevron.right").font(.caption.weight(.medium)).foregroundStyle(Theme.muted).padding(.top, 18)
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
        HStack(spacing: 12) {
            Circle().fill(torrent.needsAttention ? .orange : torrent.completed ? .cyan : Theme.mint).frame(width: 6, height: 6)
            VStack(alignment: .leading, spacing: 4) {
                Text(torrent.displayName).font(.system(size: 13, weight: .medium)).lineLimit(1)
                HStack(spacing: 6) {
                    Text(torrent.stateName)
                    Text("·")
                    Text("\(bytes(torrent.downloaded))/\(bytes(torrent.size)) · \(Int(torrent.percentDone * 100))%")
                    Text("· \(torrent.peersConnected) peers")
                }.font(.system(size: 10)).foregroundStyle(Theme.muted).lineLimit(1)
                HStack(spacing: 10) {
                    if !torrent.completed { Label(rate(torrent.rateDownload), systemImage: "arrow.down") }
                    Label(rate(torrent.rateUpload), systemImage: "arrow.up")
                    Text("Ratio \(max(0, torrent.uploadRatio), specifier: "%.2f")")
                    if !torrent.completed { Text(torrent.etaText) }
                }.font(.caption2.monospacedDigit()).foregroundStyle(Theme.muted)
                if showReason, let reason = torrent.progressIssue { Text(reason).font(.caption2).foregroundStyle(.orange).lineLimit(2) }
            }
            Spacer(minLength: 0)
            if !torrent.completed {
                ProgressView(value: torrent.percentDone).frame(width: 55).tint(Theme.mint)
            }
            Image(systemName: "chevron.right").font(.system(size: 10, weight: .medium)).foregroundStyle(Theme.muted)
        }.padding(.horizontal, 13).padding(.vertical, 10)
            .background(Theme.panel, in: RoundedRectangle(cornerRadius: 9))
            .accessibilityElement(children: .combine)
    }
}

struct TorrentActivity: View {
    let torrent: Torrent
    var body: some View {
        if !torrent.completed {
            Label(rate(torrent.rateDownload), systemImage: "arrow.down").foregroundStyle(Theme.mint)
        }
        Label(rate(torrent.rateUpload), systemImage: "arrow.up").foregroundStyle(.cyan)
        Text("\(torrent.peersConnected) peers · \(torrent.peersSendingToUs) sending")
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
                Text("History is collecting on Diamond, even when the app is closed.").font(.caption2).foregroundStyle(Theme.muted)
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

#endif
