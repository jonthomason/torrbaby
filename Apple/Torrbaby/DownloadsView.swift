#if !os(tvOS)
import SwiftUI
#if os(iOS)
import UIKit
#elseif os(macOS)
import AppKit
#endif

struct DownloadsView: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
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
    private func share(_ id: UUID) { shareDownload(id) }
    var body: some View {
        List {
            SwiftUI.Section {
                HStack {
                    Label(queue.policy.useAvailableSpace ? L10n.format("%@ used · Available space", bytes(queue.used)) : L10n.format("%@ used · %@ limit", bytes(queue.used), bytes(queue.policy.limitBytes)), systemImage: "internaldrive")
                        .font(.subheadline.monospacedDigit()).foregroundStyle(Theme.muted)
                    Spacer()
                    Button { settings = true } label: { Image(systemName: "slider.horizontal.3") }.help("Download settings").accessibilityLabel("Download settings")
                }
                #if os(iOS)
                if queue.waitingForWiFi && queue.items.contains(where: { $0.state == .queued || $0.state == .transferring }) {
                    HStack {
                        Label("Downloads are waiting for Wi-Fi", systemImage: "wifi.exclamationmark").font(.subheadline)
                        Spacer()
                        Button("Allow cellular for 3 hours") { queue.allowMeteredForAWhile() }.font(.subheadline.weight(.semibold))
                    }
                } else if queue.metered, queue.meteredAllowed, let until = queue.policy.meteredUntil {
                    HStack {
                        Label(L10n.text("Cellular downloads allowed until") + " " + Date(timeIntervalSince1970: until).formatted(date: .omitted, time: .shortened), systemImage: "antenna.radiowaves.left.and.right").font(.subheadline)
                        Spacer()
                        Button("Stop now") { queue.endMeteredAllowance() }.font(.subheadline.weight(.semibold))
                    }
                }
                #endif
                if let error = queue.error {
                    HStack { Text(L10n.text(error)).foregroundStyle(.orange); Spacer(); Button { queue.error = nil } label: { Image(systemName: "xmark.circle") } }
                }
                let visible = queue.items.filter { $0.state != .removed }
                if visible.isEmpty {
                    ContentUnavailableView("No downloads", systemImage: "arrow.down.circle")
                        .frame(maxWidth: .infinity, minHeight: 320, alignment: .center)
                        .listRowSeparator(.hidden)
                }
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
                                Label(bytes(item.size), systemImage: "checkmark.circle.fill").font(.caption).foregroundStyle(Theme.accent)
                            } else {
                                Text("\(bytes(queue.received[item.id] ?? item.received)) / \(bytes(item.size))")
                                    .font(.caption.monospacedDigit()).foregroundStyle(Theme.muted)
                                Text(L10n.text(!store.connected && [.queued, .transferring].contains(item.state)
                                              ? "Waiting for connection" : (item.error ?? queue.stateLabel(item))))
                                    .font(.caption).foregroundStyle(Theme.muted)
                            }
                        }
                        Spacer(minLength: 8)
                        if item.state == .ready {
                            Image(systemName: "play.circle.fill").font(.title).frame(width: 48, height: 48).accessibilityHidden(true)
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
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .contentShape(Rectangle())
                    .onTapGesture { if item.state == .ready { play(item.id) } }
                    .accessibilityElement(children: item.state == .ready ? .combine : .contain)
                    .accessibilityAddTraits(item.state == .ready ? .isButton : [])
                    .accessibilityAction { if item.state == .ready { play(item.id) } }
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
                    .swipeActions(edge: .leading) {
                        if item.state == .ready { Button("Share") { share(item.id) }.tint(Theme.accent) }
                    }
                    .contextMenu {
                        if item.state == .ready {
                            Button { share(item.id) } label: { Label("Share with AirDrop…", systemImage: "square.and.arrow.up") }
                        }
                        Button(role: .destructive) { queue.remove(item.id) } label: { Label("Delete", systemImage: "trash") }
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
        .sheet(isPresented: $settings) { LocalizedInterface { DownloadSettingsView() } }
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
            MacPlaybackWindows.open(item, with: openWindow)
            #else
            playback = item
            #endif
        }
    }
}

private struct DownloadPoster: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
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
                    Image(systemName: isTV ? "tv" : "film").font(.title2).foregroundStyle(Theme.accent)
                }
            }.frame(width: geometry.size.width, height: geometry.size.height).clipped()
        }.task(id: url) { data = url.flatMap { try? Data(contentsOf: $0) } }
    }
}

/// Hand a finished download to AirDrop as a `.torrbaby` package another Torrbaby opens.
@MainActor func shareDownload(_ id: UUID) {
    let queue = DownloadQueue.shared
    let url: URL
    do { url = try queue.sharePackage(id) } catch { queue.error = error.localizedDescription; return }
    #if os(iOS)
    let controller = UIActivityViewController(activityItems: [url], applicationActivities: nil)
    guard let scene = UIApplication.shared.connectedScenes.compactMap({ $0 as? UIWindowScene }).first(where: { $0.activationState == .foregroundActive }),
          var top = scene.keyWindow?.rootViewController else { return }
    while let next = top.presentedViewController { top = next }
    if let popover = controller.popoverPresentationController {
        popover.sourceView = top.view
        popover.sourceRect = CGRect(x: top.view.bounds.midX, y: top.view.bounds.midY, width: 0, height: 0)
        popover.permittedArrowDirections = []
    }
    top.present(controller, animated: true)
    #elseif os(macOS)
    if let airDrop = NSSharingService(named: .sendViaAirDrop), airDrop.canPerform(withItems: [url]) { airDrop.perform(withItems: [url]) }
    else { queue.error = "AirDrop is not available on this Mac." }
    #endif
}

/// The Downloads screen's shortcut to Settings → Downloads.
struct DownloadSettingsView: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @Environment(\.dismiss) private var dismiss
    var body: some View {
        NavigationStack {
            Form { DownloadPolicySettings() }.formStyle(.grouped).canvasBackground().navigationTitle(L10n.text("Downloads"))
                .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } } }
        }
        #if os(macOS)
        .frame(width: 460, height: 420)
        #endif
    }
}

/// Settings → Downloads. Choices apply at once, as in iOS Settings; typed sizes apply when
/// editing ends or the page closes.
struct DownloadPolicySettings: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @ObservedObject private var queue = DownloadQueue.shared
    @State private var limit = 20.0
    @State private var reserve = 0.0
    @State private var loaded = false
    private var sizesValid: Bool { limit.isFinite && reserve.isFinite && limit >= 0 && reserve >= 0 && limit <= 100_000 && reserve <= 100_000 }
    private func update(_ change: (inout DownloadStorage.Policy) -> Void) {
        var policy = queue.policy; change(&policy); queue.setPolicy(policy)
    }
    private func applySizes() {
        guard loaded, sizesValid else { return }
        let limitBytes = Int64(limit * 1_000_000_000), freeBytes = Int64(reserve * 1_000_000_000)
        guard limitBytes != queue.policy.limitBytes || freeBytes != queue.policy.freeSpaceBytes else { return }
        update { $0.limitBytes = limitBytes; $0.freeSpaceBytes = freeBytes }
    }
    var body: some View {
        Group {
            SwiftUI.Section {
                Toggle("Use available space", isOn: Binding(get: { queue.policy.useAvailableSpace }, set: { value in update { $0.useAvailableSpace = value } }))
                if !queue.policy.useAvailableSpace { LabeledContent("Download limit (GB)") { TextField("Download limit (GB)", value: $limit, format: .number).labelsHidden().multilineTextAlignment(.trailing).onSubmit(applySizes) } }
                LabeledContent("Keep free (GB)") { TextField("Keep free (GB)", value: $reserve, format: .number).labelsHidden().multilineTextAlignment(.trailing).onSubmit(applySizes) }
            } header: { Text("Storage") }
            SwiftUI.Section {
                Toggle("Delete after playback", isOn: Binding(get: { queue.policy.deleteAfterPlayback }, set: { value in update { $0.deleteAfterPlayback = value } }))
            }
            #if os(iOS)
            SwiftUI.Section {
                if queue.meteredAllowed, let until = queue.policy.meteredUntil {
                    LabeledContent(L10n.text("Cellular downloads allowed until"), value: Date(timeIntervalSince1970: until).formatted(date: .omitted, time: .shortened))
                    Button("Stop now") { queue.endMeteredAllowance() }
                } else {
                    Button("Allow cellular for 3 hours") { queue.allowMeteredForAWhile() }
                }
            } header: { Text("Cellular") } footer: {
                Text("Downloads wait for Wi-Fi. Allowing cellular also covers a Personal Hotspot or a Low Data Mode network, and ends after 3 hours or when you join Wi-Fi.")
            }
            #endif
        }
        .onAppear {
            limit = Double(queue.policy.limitBytes) / 1_000_000_000
            reserve = Double(queue.policy.freeSpaceBytes) / 1_000_000_000
            loaded = true
        }
        .onDisappear(perform: applySizes)
    }
}

struct DownloadAssetControl: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    @ObservedObject private var queue = DownloadQueue.shared
    let asset: LibraryAsset
    let title: LibraryTitle
    var compact = false
    @State private var adding = false
    @State private var confirmDelete = false
    private var item: DownloadStorage.Item? {
        queue.items.first { $0.assetID == asset.id && $0.server == store.server && $0.state != .removed }
    }
    var body: some View {
        Group {
            if compact { compactControl }
            else if let item {
                if item.state == .ready {
                    HStack(spacing: 10) {
                        VStack(alignment: .leading, spacing: 4) {
                            Label("Downloaded", systemImage: "checkmark.circle.fill").foregroundStyle(Theme.accent)
                            Text(bytes(item.size)).font(.caption).foregroundStyle(Theme.muted)
                        }
                        Button { confirmDelete = true } label: { Image(systemName: "trash") }
                            .accessibilityLabel("Delete download").buttonStyle(MonoButtonStyle())
                    }
                    .confirmationDialog("Delete this download?", isPresented: $confirmDelete, titleVisibility: .visible) {
                        Button("Delete download", role: .destructive) { queue.remove(item.id) }
                    }
                } else {
                    let got = queue.received[item.id] ?? item.received
                    let moving = item.state == .transferring || item.state == .queued
                    DownloadProgressPanel(state: queue.stateLabel(item),
                                          fraction: item.size > 0 ? Double(got) / Double(item.size) : 0,
                                          detail: "\(bytes(got)) / \(bytes(item.size))", error: item.error, moving: moving) {
                        if moving { queue.pause(item.id) } else { queue.resume(item.id) }
                    }
                }
            } else {
                Button {
                    adding = true
                    Task { await queue.enqueue(asset, title: title); adding = false }
                } label: {
                    Label(LocalizedStringKey(adding ? "Preparing…" : "Download"), systemImage: "arrow.down.circle")
                        .font(.headline)
                }.libraryActionStyle().disabled(adding || !store.connected)
            }
        }
    }
    private var compactLabel: String {
        guard let item else { return adding ? "Preparing download" : "Download" }
        return item.state == .ready ? "Delete download" : "Cancel download"
    }
    private var compactControl: some View {
        Button {
            if let item {
                if item.state == .ready { confirmDelete = true }
                else { queue.remove(item.id) }
            } else {
                adding = true
                Task { await queue.enqueue(asset, title: title); adding = false }
            }
        } label: {
            if adding { ProgressView().frame(width: 44, height: 44) }
            else { Image(systemName: item == nil ? "arrow.down.circle" : item?.state == .ready ? "trash" : "xmark.circle")
                .font(.title2).frame(width: 44, height: 44) }
        }.disabled(adding || (item == nil && (!store.connected || !asset.complete)))
            .foregroundStyle(item?.state == .ready ? Color.red : Theme.accent)
            .accessibilityLabel(compactLabel).help(compactLabel)
            .confirmationDialog("Delete this device’s download?", isPresented: $confirmDelete, titleVisibility: .visible) {
                Button("Delete download", role: .destructive) { if let item { queue.remove(item.id) } }
            }
    }

}

/// Download progress in the Play/Download row: state, pause or resume, a bar and a detail line.
struct DownloadProgressPanel: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @Environment(\.libraryActionFullWidth) private var fullWidth
    let state: String
    let fraction: Double
    let detail: String
    var error: String?
    let moving: Bool
    let toggle: () -> Void
    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack {
                Text(LocalizedStringKey(state))
                Spacer(minLength: 4)
                Button(action: toggle) { Image(systemName: moving ? "pause.fill" : "arrow.down").frame(width: 28, height: 22).contentShape(Rectangle()) }
                    .buttonStyle(.plain).accessibilityLabel(moving ? "Pause download" : "Resume download")
            }.font(.caption.weight(.semibold))
            TorrentProgressGauge(value: fraction)
            Text(detail).font(.caption2.monospacedDigit()).foregroundStyle(Theme.muted)
            if let error { Text(L10n.text(error)).font(.caption2).foregroundStyle(.orange).lineLimit(2) }
        }.padding(.horizontal, 14).padding(.vertical, 8)
            .frame(minWidth: 180, maxWidth: fullWidth ? .infinity : 300)
            .background(Theme.control, in: RoundedRectangle(cornerRadius: Theme.radius))
    }
}

/// An episode's own download bar in its row; nothing once it has finished.
struct EpisodeDownloadProgress: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    @ObservedObject private var queue = DownloadQueue.shared
    let asset: LibraryAsset
    var body: some View {
        if let item = queue.items.first(where: { $0.assetID == asset.id && $0.server == store.server && $0.state != .removed && $0.state != .removing }), item.state != .ready {
            let got = queue.received[item.id] ?? item.received
            VStack(alignment: .leading, spacing: 4) {
                TorrentProgressGauge(value: item.size > 0 ? Double(got) / Double(item.size) : 0)
                HStack {
                    Text(LocalizedStringKey(queue.stateLabel(item)))
                    Spacer(minLength: 4)
                    Text("\(bytes(got)) / \(bytes(item.size))").monospacedDigit()
                }.font(.caption2).foregroundStyle(Theme.muted)
                if let error = item.error { Text(L10n.text(error)).font(.caption2).foregroundStyle(.orange).lineLimit(2) }
            }.frame(maxWidth: 420)
        }
    }
}

struct DownloadSeasonControl: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    @ObservedObject private var queue = DownloadQueue.shared
    let title: LibraryTitle
    let season: Int
    @State private var adding = false
    @Environment(\.libraryActionFullWidth) private var fullWidth
    private var pending: [LibraryAsset] {
        title.playable.filter { asset in
            asset.season == season && !queue.items.contains { $0.server == store.server && $0.assetID == asset.id && $0.state != .removed && $0.state != .removing && $0.state != .failed }
        }
    }
    /// This device's downloads for the season, finished ones included.
    private var items: [DownloadStorage.Item] {
        queue.items.filter { $0.server == store.server && $0.titleID == title.id && $0.season == season && $0.state != .removed && $0.state != .removing }
    }
    var body: some View {
        let items = items
        let unfinished = items.filter { $0.state != .ready }
        if adding || !unfinished.isEmpty {
            // One bar for the whole season: each episode counts equally, so it stays steady while episodes are still being queued.
            let total = items.count + (adding ? pending.count : 0)
            let progress = items.reduce(0.0) { sum, item in
                sum + (item.state == .ready ? 1 : item.size > 0 ? Double(queue.received[item.id] ?? item.received) / Double(item.size) : 0)
            }
            let moving = adding || unfinished.contains { $0.state == .transferring || $0.state == .queued }
            DownloadProgressPanel(state: moving ? (queue.waitingForWiFi ? "Waiting for Wi-Fi" : "Downloading") : unfinished.contains { $0.state == .failed } ? "Failed" : "Paused",
                                  fraction: total > 0 ? progress / Double(total) : 0,
                                  detail: L10n.format("%d of %d downloaded", items.count - unfinished.count, total),
                                  error: unfinished.compactMap(\.error).first, moving: moving) {
                for item in unfinished {
                    if moving { if item.state == .transferring || item.state == .queued { queue.pause(item.id) } }
                    else { queue.resume(item.id) }
                }
            }
        } else if pending.isEmpty && !items.isEmpty {
            HStack(spacing: 10) {
                Label("Downloaded", systemImage: "checkmark.circle.fill").font(.headline)
                Text(bytes(items.reduce(0) { $0 + $1.size })).font(.caption).foregroundStyle(Theme.muted)
            }.padding(.horizontal, 20).frame(minHeight: 48).frame(maxWidth: fullWidth ? .infinity : nil)
                .background(Theme.control, in: RoundedRectangle(cornerRadius: Theme.radius))
        } else { button }
    }
    private var button: some View {
        Button {
            let assets=pending
            adding=true
            Task { for asset in assets { await queue.enqueue(asset, title: title) }; adding=false }
        } label: {
            Label(adding ? L10n.text("Queuing…") : season == 0 ? L10n.text("Download Specials") : L10n.format("Download Season %lld", season), systemImage: "arrow.down.circle")
        }.libraryActionStyle().disabled(adding || pending.isEmpty || !store.connected)
    }
}

struct DownloadTitleMenu: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
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
