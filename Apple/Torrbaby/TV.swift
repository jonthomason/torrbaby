#if os(tvOS)
import SwiftUI

struct TVRootView: View {
    @EnvironmentObject var store: Store
    @Environment(\.scenePhase) var phase
    @State private var server = ""
    @State private var token = ""
    @State private var settings = false
    @State private var playback: Playback?
    var library: [Torrent] { store.torrents.filter { $0.files.contains { $0.isVideo && $0.complete } } }
    var body: some View {
        NavigationStack {
            if !store.configured || settings {
                VStack(alignment: .leading, spacing: 24) {
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
                ScrollView {
                    LazyVGrid(columns: [GridItem(.adaptive(minimum: 360), spacing: 36)], spacing: 40) {
                        ForEach(library) { torrent in
                            NavigationLink {
                                List {
                                    ForEach(Array(torrent.files.enumerated()), id: \.offset) { index, file in
                                        if file.isVideo && file.complete {
                                            Button(file.name) { play(torrent, index) }
                                        }
                                    }
                                }.navigationTitle(torrent.displayName)
                            } label: {
                                VStack(alignment: .leading, spacing: 18) {
                                    Image(systemName: "film.stack").font(.system(size: 70)).frame(maxWidth: .infinity, minHeight: 150)
                                    Text(store.metadata.first { $0.hash == torrent.id }?.selected?.title ?? torrent.displayName).font(.headline).lineLimit(2)
                                }.padding(24).frame(height: 280)
                            }.buttonStyle(.card)
                        }
                    }.padding(50)
                }
                .navigationTitle("Torrbaby")
                .toolbar { Button("Settings") { settings = true } }
            }
        }
        .fullScreenCover(item: $playback) { PlayerScreen(playback: $0, store: store).onExitCommand { playback = nil } }
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
    func play(_ torrent: Torrent, _ index: Int) {
        Task {
            do {
                await store.flushProgress()
                try await store.registerClient()
                let link: PlaybackLink = try await store.request("POST", "/v1/torrents/\(torrent.id)/play", body: ["file_index": index, "client_id": store.clientId])
                playback = Playback(sessionId: link.sessionId, resumePosition: link.resumePosition ?? 0, url: try store.endpoint(link.path), name: torrent.displayName)
            } catch { store.actionError = error.localizedDescription }
        }
    }
}
#endif
