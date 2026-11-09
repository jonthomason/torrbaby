import SwiftUI

struct TorrentDetail: View {
    @EnvironmentObject var store: Store
    let hash: String
    @State private var showRemove = false
    @State private var showRename = false
    @State private var playback: Playback?
    @State private var loadingPlayback = false
    var torrent: Torrent? { store.torrents.first { $0.id == hash } }
    var body: some View {
        Group {
            if let t = torrent {
                ScrollView {
                    VStack(alignment: .leading, spacing: 22) {
                        HStack(alignment: .top, spacing: 18) {
                            Image(systemName: "film.stack.fill").font(.system(size: 38)).foregroundStyle(Theme.mint)
                                .frame(width: 78, height: 94).background(Theme.mint.opacity(0.08), in: RoundedRectangle(cornerRadius: 18))
                            VStack(alignment: .leading, spacing: 9) {
                                Text(t.displayName).font(.system(size: 25, weight: .bold, design: .rounded)).textSelection(.enabled)
                                Text(t.stateName).font(.subheadline.weight(.medium)).foregroundStyle(t.needsAttention ? .orange : Theme.mint)
                                Text("\(bytes(t.size)) · \(t.files.count) files").font(.caption).foregroundStyle(Theme.muted)
                            }
                        }
                        if t.error != 0 { Label(t.errorString, systemImage: "exclamationmark.triangle").font(.callout).foregroundStyle(.orange) }
                        Card {
                            VStack(alignment: .leading, spacing: 16) {
                                HStack {
                                    Text(t.completed ? "Download complete" : "Download progress").font(.headline)
                                    Spacer()
                                    Text(t.percentDone, format: .percent.precision(.fractionLength(1))).monospacedDigit().foregroundStyle(Theme.mint)
                                }
                                ProgressView(value: t.percentDone)
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
                        if let job = store.status?.jobs[hash] {
                            Card {
                                VStack(alignment: .leading, spacing: 10) {
                                    Label("Organizer · \(job.phase.capitalized)", systemImage: "sparkles").font(.headline)
                                    if let error = job.error { Text(error).font(.callout).foregroundStyle(.orange) }
                                    if let destination = job.destination { Text(destination).font(.caption).foregroundStyle(Theme.muted).textSelection(.enabled) }
                                    HStack {
                                        Button("Retry organization") { store.action(t, "retry") }
                                        Button("Ignore") { store.action(t, "ignore") }.disabled(job.phase == "moving")
                                    }.buttonStyle(.bordered).disabled(store.busy)
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
                                HStack(spacing: 12) {
                                    Image(systemName: file.isVideo ? "play.rectangle" : "doc").foregroundStyle(file.isVideo ? Theme.mint : Theme.muted)
                                    VStack(alignment: .leading, spacing: 5) {
                                        Text(file.name).font(.callout).textSelection(.enabled)
                                        Text(bytes(file.length) + (file.complete ? "" : " · Incomplete")).font(.caption).foregroundStyle(Theme.muted)
                                    }
                                    Spacer()
                                    if file.isVideo && file.complete {
                                        Button { play(t, index) } label: { Image(systemName: "play.fill").padding(5) }
                                            .buttonStyle(.bordered).help("Play original file")
                                            .accessibilityLabel("Play \(file.name)")
                                            .disabled(loadingPlayback || !store.connected)
                                    }
                                }.padding(16).background(Theme.panel, in: RoundedRectangle(cornerRadius: 13))
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
                .sheet(isPresented: $showRemove) { RemoveTorrentView(torrent: t) }
                .sheet(isPresented: $showRename) { RenameView(torrent: t) }
            } else {
                ContentUnavailableView("Torrent removed", systemImage: "checkmark.circle", description: Text("This torrent is no longer in Transmission."))
            }
        }
        .navigationTitle("Torrent details")
        .sheet(item: $playback) { PlayerScreen(playback: $0) }
    }
    @ViewBuilder func transferStats(_ t: Torrent) -> some View {
        Label(rate(t.rateDownload), systemImage: "arrow.down").foregroundStyle(Theme.mint)
        Label(rate(t.rateUpload), systemImage: "arrow.up").foregroundStyle(.cyan)
        Text("\(t.peersConnected) peers").foregroundStyle(Theme.muted)
        Text("Ratio \(max(0, t.uploadRatio), specifier: "%.2f")").foregroundStyle(Theme.muted)
    }
    @ViewBuilder func actions(_ t: Torrent) -> some View {
        Button { store.action(t, t.status == 0 ? "start" : "stop") } label: {
            Label(t.status == 0 ? "Resume" : "Pause", systemImage: t.status == 0 ? "play.fill" : "pause.fill")
        }.buttonStyle(.borderedProminent).foregroundStyle(Theme.canvas)
        Menu {
            Button("Verify downloaded data") { store.action(t, "verify") }
            Button("Reannounce to trackers") { store.action(t, "reannounce") }
            Button("Retry organization") { store.action(t, "retry") }
            Button("Ignore for organization") { store.action(t, "ignore") }
        } label: { Label("More", systemImage: "ellipsis") }.menuStyle(.borderlessButton)
        Button { showRename = true } label: { Label("Rename", systemImage: "pencil") }.buttonStyle(.bordered).disabled(!t.completed)
        Button(role: .destructive) { showRemove = true } label: { Label("Remove", systemImage: "trash") }.buttonStyle(.bordered)
    }
    func play(_ t: Torrent, _ index: Int) {
        loadingPlayback = true
        Task {
            defer { loadingPlayback = false }
            do {
                let link: PlaybackLink = try await store.request("POST", "/v1/torrents/\(t.id)/play", body: ["file_index": index])
                playback = Playback(url: try store.endpoint(link.path), name: t.displayName)
            } catch { store.actionError = error.localizedDescription }
        }
    }
}

struct RemoveTorrentView: View {
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) var dismiss
    let torrent: Torrent
    @State private var deleteData = false
    @State private var confirmation = false
    var body: some View {
        NavigationStack {
            Form {
                SwiftUI.Section {
                    Text(torrent.displayName).font(.headline)
                    Text("Removing stops this torrent and removes it from Transmission.").foregroundStyle(.secondary)
                }
                SwiftUI.Section {
                    Toggle("Also delete downloaded files", isOn: $deleteData)
                    Text(deleteData ? "The files will be permanently deleted from your server and disappear from your library." : "Downloaded files stay on your server. They can still be used by Plex.")
                        .font(.callout).foregroundStyle(deleteData ? .orange : .secondary)
                }
            }.formStyle(.grouped)
            .navigationTitle("Remove torrent")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Remove", role: .destructive) { confirmation = true }.disabled(store.busy)
                }
            }
            .confirmationDialog(deleteData ? "Permanently delete these files?" : "Remove this torrent and keep its files?", isPresented: $confirmation, titleVisibility: .visible) {
                Button(deleteData ? "Delete torrent and files" : "Remove torrent, keep files", role: .destructive) {
                    store.perform {
                        try await store.send("POST", "/v1/torrents/\(torrent.id)/remove", body: ["delete_data": deleteData, "confirm_hash": torrent.id])
                        dismiss()
                    }
                }
            }
        }.presentationDetents([.medium, .large])
        #if os(macOS)
        .frame(width: 520, height: 350)
        #endif
    }
}

struct RenameView: View {
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
                    Text("Seeding is preserved. A folder rename keeps the filenames inside it. Keep file extensions and season/episode numbers.").font(.caption).foregroundStyle(.secondary)
                }
                SwiftUI.Section("Find the proper title") {
                    Button {
                        searching = true
                        Task {
                            defer { searching = false }
                            do { result = try await store.request("POST", "/v1/torrents/\(torrent.id)/lookup"); error = nil }
                            catch { self.error = error.localizedDescription }
                        }
                    } label: { Label(searching ? "Looking up…" : "Search online", systemImage: "magnifyingglass") }.disabled(searching)
                    Text("Sends the title to TVmaze or Apple’s movie catalog. Review the year and episode range before applying a suggestion.")
                        .font(.caption).foregroundStyle(.secondary)
                    if let error { Text(error).foregroundStyle(.orange) }
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
                        Text((result.provider ?? "") + " · " + result.note).font(.caption2).foregroundStyle(.secondary)
                    }
                }
            }.formStyle(.grouped)
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
