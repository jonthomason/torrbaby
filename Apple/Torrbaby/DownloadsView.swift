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
    @AppStorage("downloads.swipeHintSeen") private var swipeHintSeen = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var hintRow: UUID?
    @State private var hintOffset: CGFloat = 0
    var body: some View {
        List {
            SwiftUI.Section {
                HStack {
                    Label(queue.policy.useAvailableSpace ? "\(bytes(queue.used)) used · Available space" : "\(bytes(queue.used)) used · \(bytes(queue.policy.limitBytes)) limit", systemImage: "internaldrive")
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
                    VStack(alignment: .leading, spacing: 8) {
                      HStack(spacing: 16) {
                        DownloadPoster(url: queue.artworkURL(item.id), isTV: item.season != nil)
                            .frame(width: 40, height: 60).clipShape(RoundedRectangle(cornerRadius: 8))
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
                                Text("\(bytes(queue.received[item.id] ?? item.received)) / \(bytes(item.size))")
                                    .font(.caption.monospacedDigit()).foregroundStyle(Theme.muted)
                                Text(item.error ?? item.state.rawValue.capitalized).font(.caption).foregroundStyle(Theme.muted)
                            }
                        }
                        Spacer(minLength: 8)
                        if item.state == .ready {
                            Button { play(item.id) } label: { Image(systemName: "play.circle.fill").font(.title).frame(width: 48, height: 48).contentShape(Rectangle()) }.buttonStyle(.borderless).accessibilityLabel("Play download").help("Play download").disabled(preparing)
                        } else if item.state == .transferring || item.state == .queued {
                            Button { queue.pause(item.id) } label: { Image(systemName: "pause.circle").font(.title2) }.help("Pause download")
                        } else {
                            Button { queue.resume(item.id) } label: { Image(systemName: "arrow.down.circle").font(.title2) }.help("Resume download")
                        }

                      }
                      if item.state != .ready {
                          TorrentProgressGauge(value: item.size > 0 ? Double(queue.received[item.id] ?? item.received) / Double(item.size) : 0)
                      }
                    }.padding(.vertical, 6)
                    .background(Theme.panel)
                    .offset(x: hintRow == item.id ? hintOffset : 0)
                    .background(alignment: .trailing) {
                        if hintRow == item.id {
                            Text("Delete").font(.subheadline.weight(.semibold)).foregroundStyle(.white)
                                .frame(width: 76).frame(maxHeight: .infinity).background(.red)
                                .accessibilityHidden(true)
                        }
                    }
                    .clipped()
                    .allowsHitTesting(hintRow != item.id)
                    .accessibilityHint("Swipe left to delete this download")
                    .listRowBackground(Theme.panel)
                    .swipeActions(edge: .trailing, allowsFullSwipe: true) {
                        Button("Delete", role: .destructive) { queue.remove(item.id) }.tint(.red)
                    }
                }
            }
        }.listStyle(.plain).scrollContentBackground(.hidden).background(Theme.canvas)
        .task(id: queue.items.first(where: { $0.state != .removed })?.id) {
            guard !swipeHintSeen, let first = queue.items.first(where: { $0.state != .removed }) else { return }
            guard !reduceMotion else { swipeHintSeen = true; return }
            do {
                try await Task.sleep(for: .milliseconds(800))
                try Task.checkCancellation()
                swipeHintSeen = true
                hintRow = first.id
                defer { hintOffset = 0; hintRow = nil }
                withAnimation(.easeInOut(duration: 0.35)) { hintOffset = -76 }
                try await Task.sleep(for: .milliseconds(1100))
                withAnimation(.easeInOut(duration: 0.35)) { hintOffset = 0 }
                try await Task.sleep(for: .milliseconds(400))
            } catch { /* Leaving the page cancels the hint. */ }
        }
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
    @State private var reserve = 0.0
    @State private var useAvailableSpace = true
    @State private var wifiOnly = true
    @State private var deleteAfter = true
    var body: some View {
        NavigationStack {
            Form {
                Toggle("Use available space", isOn: $useAvailableSpace)
                if !useAvailableSpace { TextField("Download limit (GB)", value: $limit, format: .number) }
                TextField("Keep free (GB)", value: $reserve, format: .number)
                Toggle("Wi-Fi only", isOn: $wifiOnly)
                Toggle("Delete after playback", isOn: $deleteAfter)
            }.formStyle(.grouped).navigationTitle("Download storage")
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                    ToolbarItem(placement: .confirmationAction) {
                        Button("Save") {
                            var policy = queue.policy
                            policy.useAvailableSpace = useAvailableSpace
                            policy.limitBytes = Int64(limit * 1_000_000_000)
                            policy.freeSpaceBytes = Int64(reserve * 1_000_000_000)
                            policy.wifiOnly = wifiOnly; policy.deleteAfterPlayback = deleteAfter
                            queue.setPolicy(policy); dismiss()
                        }.disabled(!limit.isFinite || !reserve.isFinite || limit < 0 || reserve < 0 || limit > 100_000 || reserve > 100_000)
                    }
                }
        }.onAppear {
            useAvailableSpace = queue.policy.useAvailableSpace
            limit = Double(queue.policy.limitBytes) / 1_000_000_000
            reserve = Double(queue.policy.freeSpaceBytes) / 1_000_000_000
            wifiOnly = queue.policy.wifiOnly; deleteAfter = queue.policy.deleteAfterPlayback
        }
        #if os(macOS)
        .frame(width: 460, height: 300)
        #endif
    }
}

struct DownloadAssetControl: View {
    @EnvironmentObject var store: Store
    @ObservedObject private var queue = DownloadQueue.shared
    let asset: LibraryAsset
    let title: LibraryTitle
    @State private var adding = false
    @State private var confirmDelete = false
    private var item: DownloadStorage.Item? {
        queue.items.first { $0.assetID == asset.id && $0.server == store.server && $0.state != .removed }
    }
    var body: some View {
        Group {
            if let item {
                if item.state == .ready {
                    HStack(spacing: 10) {
                        VStack(alignment: .leading, spacing: 4) {
                            Label("Downloaded", systemImage: "checkmark.circle.fill").foregroundStyle(Theme.mint)
                            Text(bytes(item.size)).font(.caption).foregroundStyle(Theme.muted)
                        }
                        Button { confirmDelete = true } label: { Image(systemName: "trash") }
                            .accessibilityLabel("Delete download").buttonStyle(.bordered)
                    }
                    .confirmationDialog("Delete this download?", isPresented: $confirmDelete, titleVisibility: .visible) {
                        Button("Delete download", role: .destructive) { queue.remove(item.id) }
                    }
                } else {
                    VStack(alignment: .leading, spacing: 5) {
                        HStack {
                            Text(item.state == .transferring ? "Downloading" : item.state.rawValue.capitalized)
                            Spacer(minLength: 4)
                            Button {
                                if item.state == .transferring || item.state == .queued { queue.pause(item.id) }
                                else { queue.resume(item.id) }
                            } label: { Image(systemName: item.state == .transferring || item.state == .queued ? "pause.fill" : "arrow.down") }
                                .accessibilityLabel(item.state == .transferring || item.state == .queued ? "Pause download" : "Resume download")
                        }.font(.caption)
                        TorrentProgressGauge(value: item.size > 0 ? Double(queue.received[item.id] ?? item.received) / Double(item.size) : 0)
                        Text("\(bytes(queue.received[item.id] ?? item.received)) / \(bytes(item.size))")
                            .font(.caption2.monospacedDigit()).foregroundStyle(Theme.muted)
                        if let error = item.error { Text(error).font(.caption2).foregroundStyle(.orange).lineLimit(2) }
                    }.frame(minWidth: 130, maxWidth: 280)
                }
            } else {
                Button {
                    adding = true
                    Task { await queue.enqueue(asset, title: title); adding = false }
                } label: {
                    Label(adding ? "Preparing…" : "Download", systemImage: "arrow.down.circle")
                        .font(.headline).padding(8)
                }.buttonStyle(.bordered).disabled(adding || !store.connected)
            }
        }
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
