#if !os(tvOS)
import SwiftUI

struct TorrentDetail: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    let hash: String
    @State private var showRemove = false
    @State private var showRename = false
    @Environment(\.openWindow) private var openWindow
    @State private var playback: Playback?
    @State private var loadingPlayback = false
    var torrent: Torrent? { store.torrents.first { $0.id == hash } }
    var body: some View {
        Group {
            if let t = torrent {
                ScrollView {
                    VStack(alignment: .leading, spacing: 22) {
                        HStack(alignment: .top, spacing: 18) {
                            Image(systemName: "film.stack.fill").font(.system(size: 38)).foregroundStyle(Theme.accent)
                                .frame(width: 78, height: 94).background(Theme.accent.opacity(0.08), in: RoundedRectangle(cornerRadius: 10))
                            VStack(alignment: .leading, spacing: 9) {
                                Text(t.displayName).font(.system(size: 25, weight: .bold, design: .rounded)).textSelection(.enabled)
                                Text(t.stateName).font(.subheadline.weight(.medium)).foregroundStyle(t.needsAttention ? .orange : Theme.accent)
                                Text("\(bytes(t.size)) · \(t.files.count) files").font(.caption).foregroundStyle(Theme.muted)
                            }
                        }
                        if t.error != 0 { Label(t.errorString, systemImage: "exclamationmark.triangle").font(.callout).foregroundStyle(.orange) }
                        Card {
                            VStack(alignment: .leading, spacing: 16) {
                                HStack {
                                    Text(LocalizedStringKey(t.completed ? "Download complete" : "Download progress")).font(.headline)
                                    Spacer()
                                    Text(t.percentDone, format: .percent.precision(.fractionLength(1))).monospacedDigit().foregroundStyle(Theme.accent)
                                }
                                TorrentProgressGauge(value: t.percentDone)
                                ViewThatFits(in: .horizontal) {
                                    HStack(spacing: 24) { transferStats(t) }
                                    VStack(alignment: .leading, spacing: 12) { transferStats(t) }
                                }
                            }
                        }
                        ViewThatFits(in: .horizontal) {
                            HStack(spacing: 12) { actions(t) }
                            VStack(alignment: .leading, spacing: 12) { actions(t) }
                        }.disabled(store.busy || !store.connected)
                        // Progress and controls come first; what the library made of it follows.
                        MetadataCard(torrent: t)
                        TransferHistoryView(compact: false, torrentHash: t.id, uploadOnly: t.completed)
                        if let job = store.status?.jobs[hash] {
                            Card {
                                VStack(alignment: .leading, spacing: 10) {
                                    Label("Organizer · \(job.phase.capitalized)", systemImage: "sparkles").font(.headline)
                                    if let error = job.error { Text(L10n.text(error)).font(.callout).foregroundStyle(.orange) }
                                    if let destination = job.destination { Text(destination).font(.caption).foregroundStyle(Theme.muted).textSelection(.enabled) }
                                    HStack {
                                        Button("Retry organization") { store.action(t, "retry") }
                                        Button("Ignore") { store.action(t, "ignore") }.disabled(job.phase == "moving")
                                    }.buttonStyle(MonoButtonStyle()).disabled(store.busy)
                                }
                            }
                        }
                        VStack(alignment: .leading, spacing: 12) {
                            HStack {
                                Text("FILES").font(.caption.weight(.semibold)).tracking(1.5).foregroundStyle(Theme.muted)
                                Spacer()
                                Text("Original quality · Native player").font(.caption2).foregroundStyle(Theme.muted)
                            }
                            ForEach(Array(t.files.enumerated()), id: \.offset) { index, file in
                                TorrentFileRow(torrent: t, index: index, file: file, loading: loadingPlayback) { play(t, index) }
                            }
                        }
                        Card {
                            VStack(alignment: .leading, spacing: 10) {
                                Label("Location", systemImage: "folder").font(.headline)
                                Text(t.downloadDir).font(.callout).foregroundStyle(Theme.muted).textSelection(.enabled)
                                Text("Added " + Date(timeIntervalSince1970: t.addedDate).formatted(date: .abbreviated, time: .shortened)).font(.caption).foregroundStyle(Theme.muted)
                                Text(t.id).font(.caption2.monospaced()).foregroundStyle(Theme.muted).textSelection(.enabled)
                            }
                        }
                    }.padding(24).frame(maxWidth: 1000).frame(maxWidth: .infinity)
                }.background(Theme.canvas)
                .sheet(isPresented: $showRemove) { LocalizedInterface { RemoveTorrentView(torrent: t) } }
                .sheet(isPresented: $showRename) { LocalizedInterface { RenameView(torrent: t) } }
            } else {
                ContentUnavailableView("Torrent removed", systemImage: "checkmark.circle", description: Text("This torrent is no longer in Transmission."))
            }
        }
        .navigationTitle("Torrent details")
        #if os(iOS)
        .background(MobilePlayerPresentation(playback: $playback, store: store).frame(width: 0, height: 0))
        #endif
    }
    @ViewBuilder func transferStats(_ t: Torrent) -> some View {
        Label(rate(t.rateDownload), systemImage: "arrow.down").foregroundStyle(Theme.accent)
        Label(rate(t.rateUpload), systemImage: "arrow.up").foregroundStyle(Theme.muted)
        Text(verbatim: peerCount(t.peersConnected)).foregroundStyle(Theme.muted)
        Text("Ratio \(max(0, t.uploadRatio), specifier: "%.2f")").foregroundStyle(Theme.muted)
        if !t.completed { Text(t.etaText).foregroundStyle(Theme.muted) }
        Text("\(bytes(t.downloaded)) / \(bytes(t.size))").foregroundStyle(Theme.muted)
    }
    @ViewBuilder func actions(_ t: Torrent) -> some View {
        if let operation = t.fileOperation {
            Text(LocalizedStringKey(operation.label)).foregroundStyle(.secondary)
        } else {
        Button { store.action(t, t.status == 0 ? "start" : "stop") } label: {
            Label(LocalizedStringKey(t.status == 0 ? "Resume" : "Pause"), systemImage: t.status == 0 ? "play.fill" : "pause.fill")
        }.buttonStyle(MonoButtonStyle(prominent: true)).foregroundStyle(Theme.onAccent)
        // Jumps the queue: starts now, ahead of other downloads. Also in the row's long-press menu.
        if !t.completed {
            // Once chosen it reads as a state; pressing again returns it to an ordinary download.
            Button { store.action(t, t.downloadsFirst ? "unprioritize" : "prioritize") } label: {
                Label(LocalizedStringKey(t.downloadsFirst ? "Downloading first" : "Download first"), systemImage: t.downloadsFirst ? "checkmark" : "arrow.up.to.line")
            }.buttonStyle(MonoButtonStyle()).foregroundStyle(t.downloadsFirst ? Theme.accent : Color.primary)
                .accessibilityHint(Text(LocalizedStringKey(t.downloadsFirst ? "Stop downloading first" : "")))
                .disabled(store.busy || !store.connected)
        }
        Menu {
            Button("Verify downloaded data") { store.action(t, "verify") }
            Button("Reannounce to trackers") { store.action(t, "reannounce") }
            Button("Retry organization") { store.action(t, "retry") }
            Button("Ignore for organization") { store.action(t, "ignore") }
        } label: { Label("More", systemImage: "ellipsis") }.menuStyle(.borderlessButton)
        Button { showRename = true } label: { Label("Rename", systemImage: "pencil") }.buttonStyle(MonoButtonStyle()).disabled(!t.completed)
        Button(role: .destructive) { showRemove = true } label: { Label("Remove", systemImage: "trash") }.buttonStyle(MonoButtonStyle())
        }
    }
    func play(_ t: Torrent, _ index: Int) {
        loadingPlayback = true
        Task {
            defer { loadingPlayback = false }
            do {
                try store.requireStreamingConnection()
                await store.flushProgress()
                try await store.registerClient()
                let link: PlaybackLink = try await store.request("POST", "/v1/torrents/\(t.id)/play", body: ["file_index": index, "client_id": store.clientId])
                let selection = Playback(sessionId: link.sessionId, resumePosition: link.resumePosition ?? 0, url: try store.endpoint(link.path), name: t.displayName)
                #if os(macOS)
                MacPlaybackWindows.open(selection, with: openWindow)
                #else
                playback = selection
                #endif
            } catch { store.actionError = error.localizedDescription }
        }
    }
}

struct RemoveTorrentView: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) var dismiss
    let torrent: Torrent
    var body: some View {
        NavigationStack {
            Form {
                SwiftUI.Section {
                    Text(torrent.displayName).font(.headline)
                    Text("Removing stops this torrent and removes it from Transmission.").foregroundStyle(.secondary)
                }
                SwiftUI.Section {
                    Button("Remove torrent, keep files") {
                        remove(deleteData: false)
                    }
                    Text("Stop tracking the torrent in Transmission. Keep all downloaded files on Diamond.")
                        .font(.caption).foregroundStyle(.secondary)
                }
                SwiftUI.Section {
                    Button("Remove torrent and delete files", role: .destructive) {
                        remove(deleteData: true)
                    }
                    Text("Permanently delete this torrent’s downloaded files, including partial downloads.")
                        .font(.caption).foregroundStyle(.secondary)
                }
            }.formStyle(.grouped).canvasBackground()
            .disabled(store.busy || !store.connected)
            .navigationTitle("Remove torrent")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
            }
        }.presentationDetents([.medium, .large])
        #if os(macOS)
        .frame(width: 560, height: 420)
        #endif
    }
    private func remove(deleteData: Bool) {
        store.perform {
            try await store.send("POST", "/v1/torrents/\(torrent.id)/remove", body: ["delete_data": deleteData, "confirm_hash": torrent.id])
            dismiss()
        }
    }
}

struct RenameView: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) var dismiss
    let torrent: Torrent
    @State private var path = ""
    @State private var name = ""
    @State private var result: LookupResult?
    @State private var searching = false
    @State private var error: String?
    var body: some View {
        NavigationStack {
            Form {
                SwiftUI.Section("Rename through Transmission") {
                    Picker("Path", selection: Binding(get: { path }, set: { path = $0; name = ($0 as NSString).lastPathComponent })) {
                        ForEach(paths, id: \.self) { Text($0).tag($0) }
                    }
                    TextField("New name", text: $name)
                    Text("Keep file extensions and episode numbers.").font(.caption).foregroundStyle(.secondary)
                }
                SwiftUI.Section("Find the proper title") {
                    Button {
                        searching = true
                        Task {
                            defer { searching = false }
                            do { result = try await store.request("POST", "/v1/torrents/\(torrent.id)/lookup"); error = nil }
                            catch { self.error = error.localizedDescription }
                        }
                    } label: { Label(LocalizedStringKey(searching ? "Looking up…" : "Search online"), systemImage: "magnifyingglass") }.disabled(searching)
                    Text("Check the year and episodes before applying.")
                        .font(.caption).foregroundStyle(.secondary)
                    if let error { Text(L10n.text(error)).foregroundStyle(.orange) }
                    if let result {
                        ForEach(result.candidates) { candidate in
                            VStack(alignment: .leading, spacing: 8) {
                                Text(candidate.title + " (" + candidate.year + ")").font(.headline)
                                if let suggestion = candidate.proposedRootName, let candidatePath = candidate.path {
                                    Text(suggestion).font(.caption).foregroundStyle(.secondary)
                                    Button("Use this name") { path = candidatePath; name = suggestion }
                                }
                                if let link = candidate.sourceUrl, let url = URL(string: link) { Link("View source", destination: url).font(.caption) }
                            }.padding(.vertical, 5)
                        }
                        if result.candidates.isEmpty { Text("No matches. You can still enter a name above.").foregroundStyle(.secondary) }
                        Text(LocalizedStringKey((result.provider ?? "") + " · " + result.note)).font(.caption2).foregroundStyle(.secondary)
                    }
                }
            }.formStyle(.grouped).canvasBackground()
            .navigationTitle("Review a new name")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Rename") {
                        store.perform {
                            try await store.send("POST", "/v1/torrents/\(torrent.id)/rename", body: ["path": path, "name": name])
                            dismiss()
                        }
                    }.disabled(name.isEmpty || path.isEmpty || store.busy)
                }
            }
            .onAppear {
                path = paths.first ?? ""
                name = (path as NSString).lastPathComponent
            }
        }
        #if os(macOS)
        .frame(width: 640, height: 620)
        #endif
    }
    var paths: [String] {
        Array(Set(torrent.files.map { $0.name.components(separatedBy: "/").first ?? $0.name } + torrent.files.map(\.name))).sorted()
    }
}

struct PlaybackStatusButton: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    let torrent: Torrent
    let index: Int
    var history: PlaybackHistory? {
        let key = torrent.id + ":" + String(index)
        return store.history.first { $0.id == key }
    }
    var body: some View {
        Menu {
            Button(LocalizedStringKey(history?.watched == true ? "Mark unwatched" : "Mark watched")) {
                store.action(torrent, "watched", body: ["file_index": index, "watched": history?.watched != true])
            }
        } label: {
            if history?.watched == true { Image(systemName: "checkmark.circle.fill").foregroundStyle(Theme.accent) }
            else if let history, history.position > 0 { Text("\(Int(history.position / 60)) min").font(.caption) }
            else { Image(systemName: "circle") }
        }.help("Playback history")
    }
}

struct TorrentFileRow: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    let torrent: Torrent
    let index: Int
    let file: TorrentFile
    let loading: Bool
    let play: () -> Void
    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: file.isVideo ? "play.rectangle" : "doc").foregroundStyle(file.isVideo ? Theme.accent : Theme.muted)
            VStack(alignment: .leading, spacing: 5) {
                Text(file.name).font(.callout).textSelection(.enabled)
                Text(LocalizedStringKey(bytes(file.length) + (file.complete ? "" : " · Incomplete"))).font(.caption).foregroundStyle(Theme.muted)
            }
            Spacer()
            if file.isVideo && file.complete {
                PlaybackStatusButton(torrent: torrent, index: index)
                Button(action: play) { Image(systemName: "play.fill").padding(5) }
                    .buttonStyle(MonoButtonStyle()).help("Play or resume original file")
                    .accessibilityLabel("Play \(file.name)")
                    .disabled(loading || !store.connected)
            }
        }.padding(16).background(Theme.panel, in: RoundedRectangle(cornerRadius: 10))
    }
}

struct MetadataCard: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    let torrent: Torrent
    var record: MediaMetadata? { store.metadata.first { $0.hash == torrent.id } }
    var body: some View {
        Card {
            VStack(alignment: .leading, spacing: 12) {
                Label("Library metadata", systemImage: "text.book.closed").font(.headline)
                if let selected = record?.selected {
                    MetadataArtwork(hash: torrent.id, thumbnail: selected.thumbnail)
                    Text(LocalizedStringKey(selected.title + (selected.year.isEmpty ? "" : " (" + selected.year + ")"))).font(.title3.weight(.semibold))
                    Text(selected.genres.joined(separator: " · ")).foregroundStyle(Theme.muted)
                    HStack {
                        if let rating = selected.contentRating { Text(rating) }
                        if let minutes = selected.durationMinutes, minutes > 0 { Text("\(Int(minutes)) min") }
                        if !selected.episodes.isEmpty { Text("\(selected.episodes.count) episode/file records") }
                    }.font(.caption)
                    if let summary = selected.summary, !summary.isEmpty { Text(summary).font(.callout).lineLimit(5) }
                    if let link = selected.sourceUrl, let url = URL(string: link) { Link("Source: " + selected.provider, destination: url).font(.caption) }
                    else { Text("Source: " + selected.provider).font(.caption).foregroundStyle(Theme.muted) }
                } else {
                    Text(LocalizedStringKey(record?.status == "review" ? "Choose the right title" : record?.status == "no_match" ? "No confident match yet" : record?.status == "error" ? "Lookup will retry" : "Metadata is being gathered on Diamond"))
                        .foregroundStyle(Theme.muted)
                    if let record {
                        ForEach(Array(record.candidates.enumerated()), id: \.offset) { index, candidate in
                            Button(candidate.title + " (" + candidate.year + ") · " + candidate.provider) {
                                store.action(torrent, "metadata-select", body: ["candidate": index])
                            }.buttonStyle(MonoButtonStyle())
                        }
                    }
                }
                Button("Look up again") { store.action(torrent, "metadata-refresh") }.font(.caption)
            }
        }
    }
}

struct MetadataArtwork: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    let hash: String
    let thumbnail: String?
    @State private var image: Image?
    @State private var failed = false
    @State private var retry = 0
    struct Payload: Decodable { let data: String? }
    var body: some View {
        if thumbnail != nil {
            ZStack {
                RoundedRectangle(cornerRadius: 8).fill(Theme.canvas)
                if let image {
                    image.resizable().scaledToFit()
                } else if failed {
                    VStack(spacing: 8) {
                        Image(systemName: "photo")
                        Button("Retry artwork") { retry += 1 }.font(.caption)
                    }.foregroundStyle(Theme.muted)
                } else {
                    ProgressView().controlSize(.small)
                }
            }.frame(width: 120, height: 180).clipShape(RoundedRectangle(cornerRadius: 8))
            .task(id: store.server + hash + (thumbnail ?? "") + String(retry)) {
                image = nil
                failed = false
                do {
                    let payload: Payload = try await store.request("GET", "/v1/torrents/" + hash + "/artwork")
                    try Task.checkCancellation()
                    guard let encoded = payload.data, let data = Data(base64Encoded: encoded) else {
                        failed = true; return
                    }
                    #if os(macOS)
                    if let native = NSImage(data: data) { image = Image(nsImage: native) }
                    #else
                    if let native = UIImage(data: data) { image = Image(uiImage: native) }
                    #endif
                    failed = image == nil
                } catch {
                    if !Task.isCancelled { failed = true }
                }
            }
        }
    }
}

#endif
