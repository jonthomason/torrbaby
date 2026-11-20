#if !os(tvOS)
import SwiftUI

struct TorrentDetail: View {
    @ObservedObject private var activity = ServerActivity.shared
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    let hash: String
    @State private var removing: Torrent?
    @State private var showRename = false
    @State private var showDetails = false
    @Environment(\.openWindow) private var openWindow
    @State private var playback: Playback?
    @State private var loadingPlayback = false
    var torrent: Torrent? { store.torrents.first { $0.id == hash } }
    private var record: MediaMetadata? { store.metadata.first { $0.hash == hash } }
    var body: some View { statusContent.modifier(NeedsServerStatus()) }
    @ViewBuilder private var statusContent: some View {
        Group {
            if let t = torrent {
                ScrollView {
                    VStack(alignment: .leading, spacing: 18) {
                        header(t)
                        if let issue = t.progressIssue { Label(issue, systemImage: "exclamationmark.triangle").font(.callout).foregroundStyle(.orange) }
                        progress(t)
                        ViewThatFits(in: .horizontal) {
                            HStack(spacing: 10) { actions(t) }
                            VStack(alignment: .leading, spacing: 10) { actions(t) }
                        }.disabled(store.busy || !store.connected)
                        if let job = store.status?.jobs[hash] { organizer(t, job) }
                        if record?.selected == nil { MetadataCard(torrent: t) }
                        TransferHistoryView(compact: true, torrentHash: t.id, uploadOnly: t.completed)
                        files(t)
                        DisclosureGroup(isExpanded: $showDetails) {
                            VStack(alignment: .leading, spacing: 6) {
                                Text(verbatim: t.downloadDir).textSelection(.enabled)
                                Text(verbatim: L10n.format("Added %@", Date(timeIntervalSince1970: t.addedDate).formatted(date: .abbreviated, time: .shortened)))
                                Text(verbatim: t.id).font(.caption2.monospaced()).textSelection(.enabled)
                            }.font(.caption).foregroundStyle(Theme.muted).frame(maxWidth: .infinity, alignment: .leading).padding(.top, 6)
                        } label: { Text("Details").font(.caption.weight(.semibold)).foregroundStyle(Theme.muted) }
                    }.padding(20).frame(maxWidth: 900).frame(maxWidth: .infinity)
                }.background(Theme.canvas)
                .removeTorrentAlert($removing)
                .sheet(isPresented: $showRename) { LocalizedInterface { RenameView(torrent: t) } }
            } else {
                ContentUnavailableView("Torrent removed", systemImage: "checkmark.circle", description: Text("This torrent is no longer in Transmission."))
            }
        }
        .navigationTitle("Torrent details")
        #if os(iOS)
        .navigationBarTitleDisplayMode(.inline)
        .background(MobilePlayerPresentation(playback: $playback, store: store).frame(width: 0, height: 0))
        #endif
    }
    /// Poster, the title the library knows it by, the release's tags and its own name.
    private func header(_ t: Torrent) -> some View {
        let title = t.title(record?.selected)
        let tags = (t.scope.map { [SearchTag(text: $0)] } ?? []) + (t.hints?.tags ?? [])
        return HStack(alignment: .top, spacing: 14) {
            ZStack {
                Theme.accent.opacity(0.10)
                if record?.selected?.thumbnail != nil { LibraryArtwork(id: "torrent:" + t.id) }
                else { Image(systemName: t.completed ? "film" : "arrow.down").font(.title2).foregroundStyle(Theme.accent) }
            }.frame(width: 72, height: 104).clipShape(RoundedRectangle(cornerRadius: 7)).accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 6) {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Text(verbatim: title.name).font(.title3.weight(.bold)).textSelection(.enabled)
                    if let year = title.year { Text(verbatim: year).font(.subheadline).foregroundStyle(Theme.muted) }
                }
                if !tags.isEmpty { SearchTags(tags: tags) }
                Text(verbatim: t.name).font(.caption2).foregroundStyle(Theme.muted).lineLimit(2).textSelection(.enabled)
            }
        }
    }
    /// State and percentage over the labelled bar, then one line of figures.
    private func progress(_ t: Torrent) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text(LocalizedStringKey(t.stateName)).font(.subheadline.weight(.semibold)).foregroundStyle(t.needsAttention ? .orange : Theme.accent)
                Spacer()
                Text(verbatim: t.completed ? bytes(t.size) : L10n.format("%@ of %@", bytes(t.downloaded), bytes(t.size)))
                    .font(.caption.monospacedDigit()).foregroundStyle(Theme.muted)
            }
            if !t.completed { TorrentProgressGauge(value: t.percentDone) }
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 14) { figures(t) }
                VStack(alignment: .leading, spacing: 6) { figures(t) }
            }.font(.caption.monospacedDigit()).foregroundStyle(Theme.muted)
        }.padding(14).background(Theme.panel, in: RoundedRectangle(cornerRadius: 10))
    }
    @ViewBuilder private func figures(_ t: Torrent) -> some View {
        if !t.completed { Label(rate(t.rateDownload), systemImage: "arrow.down").foregroundStyle(Theme.accent) }
        Label(rate(t.rateUpload), systemImage: "arrow.up")
        Text(verbatim: peerCount(t.peersConnected))
        if t.completed { Text(verbatim: L10n.format("Ratio %@", max(0, t.uploadRatio).formatted(.number.precision(.fractionLength(2))))) }
        else { Text(verbatim: t.etaText) }
    }
    @ViewBuilder private func actions(_ t: Torrent) -> some View {
        if let operation = t.fileOperation {
            Text(LocalizedStringKey(operation.label)).foregroundStyle(.secondary)
        } else {
            Button { store.action(t, t.status == 0 ? "start" : "stop") } label: {
                Label(LocalizedStringKey(t.status == 0 ? "Resume" : "Pause"), systemImage: t.status == 0 ? "play.fill" : "pause.fill")
            }.buttonStyle(MonoButtonStyle(prominent: true)).foregroundStyle(Theme.onAccent)
            // Jumps the queue: starts now, ahead of other downloads. Pressing again makes it ordinary.
            if !t.completed {
                Button { store.action(t, t.downloadsFirst ? "unprioritize" : "prioritize") } label: {
                    Label(LocalizedStringKey(t.downloadsFirst ? "Downloading first" : "Download first"), systemImage: t.downloadsFirst ? "checkmark" : "arrow.up.to.line")
                }.buttonStyle(MonoButtonStyle()).foregroundStyle(t.downloadsFirst ? Theme.accent : Color.primary)
            }
            Menu {
                Button("Rename…") { showRename = true }.disabled(!t.completed)
                Button("Look up again") { store.action(t, "metadata-refresh") }
                Divider()
                Button("Verify downloaded data") { store.action(t, "verify") }
                Button("Reannounce to trackers") { store.action(t, "reannounce") }
                Button("Retry organization") { store.action(t, "retry") }
                Button("Ignore for organization") { store.action(t, "ignore") }
            } label: { Label("More", systemImage: "ellipsis") }.menuStyle(.borderlessButton).fixedSize()
            Button(role: .destructive) { removing = t } label: { Label("Remove", systemImage: "trash") }.buttonStyle(MonoButtonStyle())
        }
    }
    private func organizer(_ t: Torrent, _ job: Job) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Label("Organizer · \(job.phase.capitalized)", systemImage: "sparkles").font(.subheadline.weight(.semibold))
            if let error = job.error { Text(L10n.text(error)).font(.callout).foregroundStyle(.orange) }
            HStack {
                Button("Retry organization") { store.action(t, "retry") }
                Button("Ignore") { store.action(t, "ignore") }.disabled(job.phase == "moving")
            }.buttonStyle(MonoButtonStyle(compact: true)).disabled(store.busy)
        }.padding(14).frame(maxWidth: .infinity, alignment: .leading).background(Theme.panel, in: RoundedRectangle(cornerRadius: 10))
    }
    private func files(_ t: Torrent) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(t.files.count == 1 ? "1 FILE" : "\(t.files.count) FILES").font(.caption.weight(.semibold)).tracking(1.4).foregroundStyle(Theme.muted).padding(.bottom, 6)
            ForEach(Array(t.files.enumerated()), id: \.offset) { index, file in
                TorrentFileRow(torrent: t, index: index, file: file, loading: loadingPlayback) { play(t, index) }
                if index < t.files.count - 1 { Divider().opacity(0.5) }
            }
        }
    }
    func play(_ t: Torrent, _ index: Int) {
        loadingPlayback = true
        Task {
            defer { loadingPlayback = false }
            do {
                try await store.requireStreamingConnection()
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

extension View {
    /// Removing a torrent: one alert, its name, and three choices.
    func removeTorrentAlert(_ torrent: Binding<Torrent?>) -> some View { modifier(RemoveTorrentAlert(torrent: torrent)) }
}
private struct RemoveTorrentAlert: ViewModifier {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    @Binding var torrent: Torrent?
    func body(content: Content) -> some View {
        let shown = torrent
        let name = shown.map { t in t.title(store.metadata.first { $0.hash == t.id }?.selected).name } ?? ""
        content.alert(L10n.format("Remove “%@”?", name), isPresented: Binding(get: { torrent != nil }, set: { if !$0 { torrent = nil } })) {
            Button("Keep files") { remove(shown, deleteData: false) }
            Button("Delete files", role: .destructive) { remove(shown, deleteData: true) }
            Button("Cancel", role: .cancel) {}
        }
    }
    private func remove(_ torrent: Torrent?, deleteData: Bool) {
        guard let torrent else { return }
        store.perform {
            try await store.send("POST", "/v1/torrents/\(torrent.id)/remove", body: ["delete_data": deleteData, "confirm_hash": torrent.id])
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
        HStack(spacing: 10) {
            Image(systemName: file.isVideo ? "play.rectangle" : "doc").foregroundStyle(file.isVideo ? Theme.accent : Theme.muted)
            VStack(alignment: .leading, spacing: 2) {
                Text(verbatim: (file.name as NSString).lastPathComponent).font(.footnote).textSelection(.enabled).lineLimit(2)
                Text(LocalizedStringKey(bytes(file.length) + (file.complete ? "" : " · Incomplete"))).font(.caption2).foregroundStyle(Theme.muted)
            }
            Spacer()
            if file.isVideo && file.complete {
                PlaybackStatusButton(torrent: torrent, index: index)
                Button(action: play) { Image(systemName: "play.fill").padding(5) }
                    .buttonStyle(MonoButtonStyle()).help("Play or resume original file")
                    .accessibilityLabel("Play \(file.name)")
                    .disabled(loading || !store.connected)
            }
        }.padding(.vertical, 8)
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
