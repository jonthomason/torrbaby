#if !os(tvOS)
import SwiftUI

struct DownloadsView: View {
    @ObservedObject private var queue = DownloadQueue.shared
    @EnvironmentObject var store: Store
    #if os(macOS)
    @Environment(\.openWindow) private var openWindow
    #endif
    @State private var playback: Playback?
    @State private var settings = false
    @State private var preparing = false
    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 12) {
                HStack {
                    Label("\(bytes(queue.used)) / \(bytes(queue.policy.limitBytes))", systemImage: "internaldrive")
                        .font(.subheadline.monospacedDigit()).foregroundStyle(Theme.muted)
                    Spacer()
                    Button { settings = true } label: { Image(systemName: "slider.horizontal.3") }.help("Download storage")
                }
                if let error = queue.error {
                    HStack { Text(error).foregroundStyle(.orange); Spacer(); Button { queue.error = nil } label: { Image(systemName: "xmark.circle") } }
                }
                let visible = queue.items.filter { $0.state != .removed }
                if visible.isEmpty { ContentUnavailableView("No downloads", systemImage: "arrow.down.circle") }
                ForEach(visible) { item in
                    HStack(spacing: 16) {
                        DownloadPoster(url: queue.artworkURL(item.id), isTV: item.season != nil)
                            .frame(width: 60, height: 90).clipShape(RoundedRectangle(cornerRadius: 8))
                        VStack(alignment: .leading, spacing: 6) {
                            Text(item.title).font(.headline).lineLimit(2)
                            if let season = item.season, let episode = item.episode {
                                Text(String(format: "S%02d E%02d", season, episode)).font(.caption.monospacedDigit()).foregroundStyle(Theme.muted)
                            } else if item.name != item.title {
                                Text(item.name).font(.caption).foregroundStyle(Theme.muted).lineLimit(1)
                            }
                            if item.state == .ready {
                                Label(bytes(item.size), systemImage: "checkmark.circle.fill").font(.caption).foregroundStyle(Theme.mint)
                            } else {
                                ProgressView(value: Double(queue.received[item.id] ?? item.received), total: Double(item.size))
                                Text("\(bytes(queue.received[item.id] ?? item.received)) / \(bytes(item.size))")
                                    .font(.caption.monospacedDigit()).foregroundStyle(Theme.muted)
                                Text(item.error ?? item.state.rawValue.capitalized).font(.caption).foregroundStyle(Theme.muted)
                            }
                        }
                        Spacer(minLength: 8)
                        if item.state == .ready {
                            Button { play(item.id) } label: { Image(systemName: "play.circle.fill").font(.title) }.help("Play download").disabled(preparing)
                        } else if item.state == .transferring || item.state == .queued {
                            Button { queue.pause(item.id) } label: { Image(systemName: "pause.circle").font(.title2) }.help("Pause download")
                        } else {
                            Button { queue.resume(item.id) } label: { Image(systemName: "arrow.down.circle").font(.title2) }.help("Resume download")
                        }
                        Menu {
                            Toggle("Keep downloaded", isOn: Binding(get: { item.pinned }, set: { queue.pin(item.id, keep: $0) }))
                            Button("Remove local download", role: .destructive) { queue.remove(item.id) }
                        } label: { Image(systemName: "ellipsis.circle") }.menuStyle(.borderlessButton)
                    }.padding(18).background(Theme.panel, in: RoundedRectangle(cornerRadius: 14))
                }
            }.padding(24)
        }.background(Theme.canvas)
        .sheet(isPresented: $settings) { DownloadSettingsView() }
        #if os(iOS)
        .background(MobilePlayerPresentation(playback: $playback, store: store).frame(width: 0, height: 0))
        #endif
    }
    private func play(_ id: UUID) {
        guard !preparing else { return }
        preparing = true
        Task {
            defer { preparing = false }
            guard let item = await queue.preparePlayback(id) else { return }
            #if os(macOS)
            openWindow(value: item)
            #else
            playback = item
            #endif
        }
    }
}

private struct DownloadPoster: View {
    let url: URL?
    let isTV: Bool
    @State private var data: Data?
    var body: some View {
        GeometryReader { geometry in
            ZStack {
                Theme.panel
                if let data {
                    #if os(macOS)
                    if let image = NSImage(data: data) { Image(nsImage: image).resizable().scaledToFill() }
                    #else
                    if let image = UIImage(data: data) { Image(uiImage: image).resizable().scaledToFill() }
                    #endif
                } else {
                    Image(systemName: isTV ? "tv" : "film").font(.title2).foregroundStyle(Theme.mint)
                }
            }.frame(width: geometry.size.width, height: geometry.size.height).clipped()
        }.task(id: url) { data = url.flatMap { try? Data(contentsOf: $0) } }
    }
}

struct DownloadSettingsView: View {
    @ObservedObject private var queue = DownloadQueue.shared
    @Environment(\.dismiss) private var dismiss
    @State private var limit = 20.0
    @State private var reserve = 5.0
    @State private var wifiOnly = true
    @State private var deleteAfter = true
    var body: some View {
        NavigationStack {
            Form {
                TextField("Download limit (GB)", value: $limit, format: .number)
                TextField("Keep free (GB)", value: $reserve, format: .number)
                Toggle("Wi-Fi only", isOn: $wifiOnly)
                Toggle("Delete after playback", isOn: $deleteAfter)
            }.formStyle(.grouped).navigationTitle("Download storage")
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                    ToolbarItem(placement: .confirmationAction) {
                        Button("Save") {
                            var policy = queue.policy
                            policy.limitBytes = Int64(limit * 1_000_000_000)
                            policy.freeSpaceBytes = Int64(reserve * 1_000_000_000)
                            policy.wifiOnly = wifiOnly; policy.deleteAfterPlayback = deleteAfter
                            queue.setPolicy(policy); dismiss()
                        }.disabled(!limit.isFinite || !reserve.isFinite || limit < 0 || reserve < 0 || limit > 100_000 || reserve > 100_000)
                    }
                }
        }.onAppear {
            limit = Double(queue.policy.limitBytes) / 1_000_000_000
            reserve = Double(queue.policy.freeSpaceBytes) / 1_000_000_000
            wifiOnly = queue.policy.wifiOnly; deleteAfter = queue.policy.deleteAfterPlayback
        }
        #if os(macOS)
        .frame(width: 460, height: 300)
        #endif
    }
}

struct DownloadTitleMenu: View {
    @EnvironmentObject var store: Store
    @ObservedObject private var queue = DownloadQueue.shared
    let title: LibraryTitle
    var body: some View {
        Menu {
            if title.kind == "tv" {
                ForEach(Array(Set(title.playable.compactMap(\.season))).sorted(), id: \.self) { season in
                    Button("Download season \(season)") { enqueue(title.playable.filter { $0.season == season }) }
                }
            }
            ForEach(title.playable) { asset in
                Button(asset.name ?? title.title) { enqueue([asset]) }
            }
        } label: { Image(systemName: "arrow.down.circle").font(.title2) }
            .help("Download to this device").disabled(title.playable.isEmpty || !store.connected)
    }
    private func enqueue(_ assets: [LibraryAsset]) {
        Task { for asset in assets { await queue.enqueue(asset, title: title) } }
    }
}
#endif
