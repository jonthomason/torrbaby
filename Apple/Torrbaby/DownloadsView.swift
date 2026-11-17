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
    @State private var confirmingCancel = false
    @AppStorage("downloads.swipeHintSeen") private var swipeHintSeen = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var hintRow: String?
    @State private var hintOffset: CGFloat = 0
    private func share(_ id: UUID) { shareDownload(id) }
    /// The device's free space; "< 1 MB" when it is all but gone, rather than "Zero KB".
    private var freeText: String {
        guard let free = queue.free else { return "—" }
        return free < 1_000_000 ? L10n.text("< 1 MB") : bytes(free)
    }
    var body: some View {
        List {
            SwiftUI.Section {
                HStack {
                    Label(queue.policy.useAvailableSpace ? L10n.format("%@ used · %@ free", bytes(queue.used), freeText) : L10n.format("%@ used · %@ limit", bytes(queue.used), bytes(queue.policy.limitBytes)), systemImage: "internaldrive")
                        .font(.subheadline.monospacedDigit()).foregroundStyle(Theme.muted)
                    Spacer()
                    Button { settings = true } label: { Image(systemName: "slider.horizontal.3") }.help("Download settings").accessibilityLabel("Download settings")
                }
                // Downloads that will not fit are said before they stop.
                if queue.spaceStop == nil, let free = queue.free, queue.remaining > free - queue.policy.keepFree {
                    Label(L10n.format("These downloads need %@ more than this device has free. Delete downloads or free up space, or cancel some.", bytes(queue.remaining - (free - queue.policy.keepFree))),
                          systemImage: "exclamationmark.triangle")
                        .font(.subheadline).foregroundStyle(.orange)
                        .fixedSize(horizontal: false, vertical: true)
                }
                // Downloads stopped for space say so above everything else, with what to do.
                if let stop = queue.spaceStop {
                    Label(stop == DownloadStorage.limitReached
                          ? L10n.text("Downloads stopped: they reached the storage limit in Download settings. Raise the limit or delete downloads; they continue when there is room.")
                          : L10n.text("Downloads stopped: this device ran out of space. Space shown as free can include caches iOS has not cleared yet. Delete downloads or free up space; they try again by themselves when there is room."),
                          systemImage: "externaldrive.badge.exclamationmark")
                        .font(.subheadline.weight(.semibold)).foregroundStyle(.orange)
                        .fixedSize(horizontal: false, vertical: true)
                }
                // Every download at once: resume, pause, or cancel what is not finished.
                if !queue.unfinished.isEmpty {
                    HStack(spacing: 18) {
                        if queue.items.contains(where: { $0.state == .paused || $0.state == .failed }) || queue.stalled {
                            Button { queue.resumeAll() } label: { Label("Resume all", systemImage: "arrow.down.circle") }.help("Resume all downloads")
                        }
                        if queue.items.contains(where: { $0.state == .queued || $0.state == .transferring }) {
                            Button { queue.pauseAll() } label: { Label("Pause all", systemImage: "pause.circle") }.help("Pause all downloads")
                        }
                        do {
                            Button(role: .destructive) { confirmingCancel = true } label: { Label("Cancel all", systemImage: "xmark.circle") }.help("Cancel all downloads")
                                .confirmationDialog(L10n.format("Cancel %lld downloads?", queue.unfinished.count), isPresented: $confirmingCancel, titleVisibility: .visible) {
                                    Button("Cancel all downloads", role: .destructive) { queue.cancelAll() }
                                    Button("Keep downloading", role: .cancel) {}
                                } message: {
                                    Text("Downloads not finished yet are removed from this device. Finished downloads stay.")
                                }
                        }
                        Spacer()
                    }.buttonStyle(.borderless).font(.subheadline.weight(.semibold))
                }
                #if os(iOS)
                // What the episodes fetched by themselves take, against their own limit.
                if !AutoDownload.followed.isEmpty {
                    Label(L10n.format("New episodes: %@ of %@", bytes(AutoDownload.usedBytes), bytes(AutoDownload.limitBytes)), systemImage: "arrow.down.circle")
                        .font(.subheadline.monospacedDigit()).foregroundStyle(Theme.muted)
                }
                #endif
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
                // Every download with updated subtitles, updated with one button.
                if visible.filter({ queue.updates[$0.id] != nil }).count > 1 {
                    DownloadUpdateOffer(ids: visible.map(\.id))
                }
                if visible.isEmpty {
                    ContentUnavailableView("No downloads", systemImage: "arrow.down.circle")
                        .frame(maxWidth: .infinity, minHeight: 320, alignment: .center)
                        .listRowSeparator(.hidden)
                }
                // By show and by movie (the user, 2026-11-17): a show is one entry with the episode
                // being watched, a movie one entry with its progress. A show opens its episodes.
                ForEach(DownloadGroup.groups(visible)) { group in
                    DownloadGroupRow(group: group, play: play)
                        .offset(x: hintRow == group.id ? hintOffset : 0)
                        .background(alignment: .trailing) {
                            if hintRow == group.id {
                                Text("Delete").font(.subheadline.weight(.semibold)).foregroundStyle(.white)
                                    .frame(width: 76).frame(maxHeight: .infinity).background(.red)
                                    .accessibilityHidden(true)
                            }
                        }
                        .clipped()
                        .allowsHitTesting(hintRow != group.id)
                        .listRowBackground(Theme.panel)
                        .swipeActions(edge: .trailing, allowsFullSwipe: true) {
                            Button("Delete", role: .destructive) { for item in group.items { queue.remove(item.id) } }.tint(.red)
                        }
                        .swipeActions(edge: .leading) {
                            if !group.isShow, let item = group.items.first, item.state == .ready { Button("Share") { share(item.id) }.tint(Theme.accent) }
                        }
                }
            }
        }.listStyle(.plain).scrollContentBackground(.hidden).background(Theme.canvas)
        // Opening this page is one of the two moments downloads are checked for updates
        // (the other is a title's own page). Nothing checks in the background.
        .task(id: store.connected && store.library != nil) { if let library = store.library { await queue.checkForUpdates(library: library) } }
        .task(id: DownloadGroup.groups(queue.items.filter { $0.state != .removed }).first?.id) {
            guard !swipeHintSeen, let first = DownloadGroup.groups(queue.items.filter { $0.state != .removed }).first else { return }
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

/// One show's or one movie's downloads on this device, for the Downloads page.
struct DownloadGroup: Identifiable {
    let id: String
    let items: [DownloadStorage.Item]
    var first: DownloadStorage.Item { items[0] }
    var isShow: Bool { items.contains { $0.season != nil } }
    var ready: [DownloadStorage.Item] { items.filter { $0.state == .ready } }
    var size: Int64 { items.reduce(0) { $0 + $1.size } }
    /// Every downloaded episode (or the movie) is watched.
    var allWatched: Bool { !ready.isEmpty && ready.allSatisfy(\.watched) }
    var watchedReady: [DownloadStorage.Item] { ready.filter(\.watched) }
    var unfinished: [DownloadStorage.Item] { items.filter { $0.state != .ready } }
    /// The episode to show and play: the one being watched (latest progress), else the first
    /// not watched, else the last.
    var current: DownloadStorage.Item {
        let ordered = ready.isEmpty ? items : ready
        if let watching = ordered.filter({ $0.position > 0 && !$0.watched }).max(by: { ($0.progressRecordedAt ?? .distantPast) < ($1.progressRecordedAt ?? .distantPast) }) { return watching }
        return ordered.first { !$0.watched } ?? ordered.last ?? first
    }
    /// Shows and movies in the order their first download was added; a show's episodes by number.
    static func groups(_ items: [DownloadStorage.Item]) -> [DownloadGroup] {
        var order: [String] = []
        var grouped: [String: [DownloadStorage.Item]] = [:]
        for item in items {
            let key = item.server + "|" + item.titleID
            if grouped[key] == nil { order.append(key) }
            grouped[key, default: []].append(item)
        }
        return order.map { key in
            DownloadGroup(id: key, items: grouped[key]!.sorted { ($0.season ?? 0, $0.episode ?? 0) < ($1.season ?? 0, $1.episode ?? 0) })
        }
    }
}

/// Played so far (and whether watched) as a bar, as on a title's page.
private struct DownloadWatchProgress: View {
    let item: DownloadStorage.Item
    var body: some View {
        if !item.watched, item.duration > 0, item.position > 0 {
            ProgressView(value: min(1, item.position / item.duration)).tint(Theme.accent)
        }
    }
}

/// A show or a movie on the Downloads page: its poster and name, the episode being watched
/// and how far, whether it is all watched, the space it takes, and buttons to play it, delete
/// the watched episodes (a show) and delete it all (x). A show opens its episodes.
private struct DownloadGroupRow: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @ObservedObject private var queue = DownloadQueue.shared
    @EnvironmentObject var store: Store
    let group: DownloadGroup
    let play: (UUID) -> Void
    @State private var confirmingDelete = false
    var body: some View {
        if group.isShow {
            NavigationLink { DownloadedShowView(groupID: group.id, play: play) } label: { content }
        } else {
            content.contentShape(Rectangle())
                .onTapGesture { if group.first.state == .ready { play(group.first.id) } }
                .accessibilityAddTraits(group.first.state == .ready ? .isButton : [])
                .accessibilityAction { if group.first.state == .ready { play(group.first.id) } }
        }
    }
    /// The whole series is watched on the server too (unknown away from the library: not claimed).
    private var seriesWatched: Bool {
        guard let title = store.library?.titles.first(where: { $0.id == group.first.titleID }), store.libraryKey == group.first.server else { return false }
        return !title.playable.isEmpty && title.playable.allSatisfy(\.watched)
    }
    private var content: some View {
        let current = group.current
        return VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 16) {
                DownloadPoster(url: queue.artworkURL(current.id), isTV: group.isShow)
                    .frame(width: 40, height: 60).clipShape(RoundedRectangle(cornerRadius: 8))
                VStack(alignment: .leading, spacing: 6) {
                    HStack(spacing: 6) {
                        Text(verbatim: current.title).font(.headline).lineLimit(2)
                        if group.allWatched { Image(systemName: "checkmark.circle.fill").foregroundStyle(Theme.accent).accessibilityLabel(Text("Watched")) }
                    }
                    if group.isShow {
                        if let line = queue.episodeLine(current) { Text(verbatim: line).font(.caption).foregroundStyle(Theme.muted).lineLimit(2) }
                        // Watched means the episodes on this device, not the whole show.
                        if group.allWatched && !seriesWatched {
                            Text("Downloaded episodes watched").font(.caption2).foregroundStyle(Theme.muted)
                        }
                    }
                    DownloadWatchProgress(item: current)
                    HStack(spacing: 8) {
                        if group.isShow { Text(L10n.format("%lld episodes", group.items.count)) }
                        Text(verbatim: bytes(group.size))
                        if !group.unfinished.isEmpty {
                            Text(L10n.format("%lld downloading", group.unfinished.count))
                        }
                    }.font(.caption.monospacedDigit()).foregroundStyle(Theme.muted)
                    if !group.isShow, current.state != .ready {
                        Text(L10n.text(!store.connected && [.queued, .transferring].contains(current.state) ? "Waiting for connection" : (current.error ?? queue.stateLabel(current))))
                            .font(.caption).foregroundStyle(Theme.muted)
                    }
                }
                Spacer(minLength: 8)
                if group.isShow && !group.watchedReady.isEmpty && !group.allWatched {
                    Button { for item in group.watchedReady { queue.remove(item.id) } } label: {
                        Image(systemName: "checkmark.circle.badge.xmark").font(.title2)
                    }.buttonStyle(.borderless).help("Delete watched episodes").accessibilityLabel(Text("Delete watched episodes"))
                }
                if current.state == .ready {
                    Button { play(current.id) } label: { Image(systemName: "play.circle.fill").font(.title) }
                        .buttonStyle(.borderless).help("Play").accessibilityLabel(Text("Play"))
                } else if !group.isShow {
                    if current.state == .transferring || current.state == .queued {
                        Button { queue.pause(current.id) } label: { Image(systemName: "pause.circle").font(.title2) }.buttonStyle(.borderless).help("Pause download")
                    } else {
                        Button { queue.resume(current.id) } label: { Image(systemName: "arrow.down.circle").font(.title2) }.buttonStyle(.borderless).help("Resume download")
                    }
                }
                Button { if group.isShow { confirmingDelete = true } else { for item in group.items { queue.remove(item.id) } } } label: {
                    Image(systemName: "xmark.circle").font(.title2).foregroundStyle(.secondary)
                }.buttonStyle(.borderless).help(group.isShow ? "Delete all downloaded episodes" : "Delete download")
                .accessibilityLabel(Text(group.isShow ? "Delete all downloaded episodes" : "Delete download"))
                .confirmationDialog(L10n.format("Delete all %lld downloaded episodes of %@?", group.items.count, current.title), isPresented: $confirmingDelete, titleVisibility: .visible) {
                    Button("Delete all downloaded episodes", role: .destructive) { for item in group.items { queue.remove(item.id) } }
                    Button("Keep", role: .cancel) {}
                }
            }
            if !group.unfinished.isEmpty {
                let received = group.items.reduce(Int64(0)) { $0 + (queue.received[$1.id] ?? ($1.state == .ready ? $1.size : $1.received)) }
                TorrentProgressGauge(value: group.size > 0 ? Double(received) / Double(group.size) : 0)
            }
            if !group.isShow { DownloadUpdateOffer(ids: [group.first.id]).padding(.leading, 56) }
        }.padding(.vertical, 6).frame(maxWidth: .infinity, alignment: .leading).background(Theme.panel)
    }
}

/// A show's downloaded episodes, by season: each with its progress and watched mark, played by
/// a tap, deleted with its x or a swipe; above them, delete the watched ones or all of them.
private struct DownloadedShowView: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @ObservedObject private var queue = DownloadQueue.shared
    @EnvironmentObject var store: Store
    let groupID: String
    let play: (UUID) -> Void
    @State private var confirmingDelete = false
    var body: some View {
        let group = DownloadGroup.groups(queue.items.filter { $0.state != .removed }).first { $0.id == groupID }
        List {
            if let group {
                SwiftUI.Section {
                    HStack(spacing: 18) {
                        if !group.watchedReady.isEmpty {
                            Button { for item in group.watchedReady { queue.remove(item.id) } } label: { Label("Delete watched episodes", systemImage: "checkmark.circle.badge.xmark") }
                        }
                        Button(role: .destructive) { confirmingDelete = true } label: { Label("Delete all", systemImage: "xmark.circle") }
                            .confirmationDialog(L10n.format("Delete all %lld downloaded episodes of %@?", group.items.count, group.first.title), isPresented: $confirmingDelete, titleVisibility: .visible) {
                                Button("Delete all downloaded episodes", role: .destructive) { for item in group.items { queue.remove(item.id) } }
                                Button("Keep", role: .cancel) {}
                            }
                        Spacer()
                    }.buttonStyle(.borderless).font(.subheadline.weight(.semibold))
                    if group.ready.filter({ queue.updates[$0.id] != nil }).count > 1 { DownloadUpdateOffer(ids: group.items.map(\.id)) }
                }
                let seasons = Array(Set(group.items.map { $0.season ?? 0 })).sorted()
                ForEach(seasons, id: \.self) { season in
                    SwiftUI.Section(season == 0 ? L10n.text("Specials") : L10n.format("Season %lld", season)) {
                        ForEach(group.items.filter { ($0.season ?? 0) == season }) { item in
                            DownloadEpisodeRow(item: item, play: play)
                                .listRowBackground(Theme.panel)
                                .swipeActions(edge: .trailing, allowsFullSwipe: true) {
                                    Button("Delete", role: .destructive) { queue.remove(item.id) }.tint(.red)
                                }
                                .swipeActions(edge: .leading) {
                                    if item.state == .ready { Button("Share") { shareDownload(item.id) }.tint(Theme.accent) }
                                }
                        }
                    }
                }
            } else {
                ContentUnavailableView("No downloads", systemImage: "arrow.down.circle").listRowSeparator(.hidden)
            }
        }.listStyle(.plain).scrollContentBackground(.hidden).background(Theme.canvas)
        .navigationTitle(group?.first.title ?? "")
    }
}

/// One downloaded episode: its number and name, progress or watched mark, size or download
/// state, and play / pause / resume and delete.
private struct DownloadEpisodeRow: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @ObservedObject private var queue = DownloadQueue.shared
    @EnvironmentObject var store: Store
    let item: DownloadStorage.Item
    let play: (UUID) -> Void
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 14) {
                VStack(alignment: .leading, spacing: 5) {
                    HStack(spacing: 6) {
                        Text(verbatim: queue.episodeLine(item) ?? item.name).font(.headline).lineLimit(2)
                        if item.watched { Image(systemName: "checkmark.circle.fill").foregroundStyle(Theme.accent).accessibilityLabel(Text("Watched")) }
                    }
                    DownloadWatchProgress(item: item)
                    if item.state == .ready {
                        Text(verbatim: bytes(item.size)).font(.caption.monospacedDigit()).foregroundStyle(Theme.muted)
                    } else {
                        Text(verbatim: "\(bytes(queue.received[item.id] ?? item.received)) / \(bytes(item.size))").font(.caption.monospacedDigit()).foregroundStyle(Theme.muted)
                        Text(L10n.text(!store.connected && [.queued, .transferring].contains(item.state) ? "Waiting for connection" : (item.error ?? queue.stateLabel(item))))
                            .font(.caption).foregroundStyle(Theme.muted)
                    }
                }
                Spacer(minLength: 8)
                if item.state == .ready {
                    Button { play(item.id) } label: { Image(systemName: "play.circle.fill").font(.title) }.buttonStyle(.borderless).help("Play").accessibilityLabel(Text("Play"))
                } else if item.state == .transferring || item.state == .queued {
                    Button { queue.pause(item.id) } label: { Image(systemName: "pause.circle").font(.title2) }.buttonStyle(.borderless).help("Pause download")
                } else {
                    Button { queue.resume(item.id) } label: { Image(systemName: "arrow.down.circle").font(.title2) }.buttonStyle(.borderless).help("Resume download")
                }
                Button { queue.remove(item.id) } label: { Image(systemName: "xmark.circle").font(.title2).foregroundStyle(.secondary) }
                    .buttonStyle(.borderless).help("Delete download").accessibilityLabel(Text("Delete download"))
            }
            if item.state != .ready {
                TorrentProgressGauge(value: item.size > 0 ? Double(queue.received[item.id] ?? item.received) / Double(item.size) : 0)
            }
            DownloadUpdateOffer(ids: [item.id])
        }.padding(.vertical, 6).frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(Rectangle())
        .onTapGesture { if item.state == .ready { play(item.id) } }
        .contextMenu {
            if item.state == .ready { Button { shareDownload(item.id) } label: { Label("Share with AirDrop…", systemImage: "square.and.arrow.up") } }
            Button(role: .destructive) { queue.remove(item.id) } label: { Label("Delete", systemImage: "trash") }
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
            AutoDownloadSettings()
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
        queue.items.first { $0.assetID == asset.id && $0.server == store.libraryKey && $0.state != .removed }
    }
    var body: some View {
        // Checked when the page opens, and again if the connection arrives after it.
        content.task(id: title.id + (store.connected ? "|connected" : "")) { await queue.checkForUpdates(title: title) }
    }
    private var content: some View {
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
                    DownloadUpdateOffer(ids: [item.id])
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
        if let item = queue.items.first(where: { $0.assetID == asset.id && $0.server == store.libraryKey && $0.state != .removed && $0.state != .removing }), item.state != .ready {
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

/// Shown beside finished downloads when the server now has subtitles they lack: one tap
/// fetches only those, not the video again.
struct DownloadUpdateOffer: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @ObservedObject private var queue = DownloadQueue.shared
    let ids: [UUID]
    var body: some View {
        let pending = ids.filter { queue.updates[$0] != nil }
        if !pending.isEmpty {
            let busy = pending.contains { queue.updating.contains($0) }
            let size = pending.reduce(Int64(0)) { $0 + (queue.updates[$1]?.bytes ?? 0) }
            VStack(alignment: .leading, spacing: 6) {
                Text(pending.count == 1 ? L10n.text("Updated subtitles are available for this download.") : L10n.format("Updated subtitles are available for %lld downloads.", pending.count)).font(.caption).foregroundStyle(Theme.muted)
                    .fixedSize(horizontal: false, vertical: true)
                Button {
                    Task { if pending.count > 1 { await queue.applyAllUpdates(pending) } else { for id in pending { await queue.applyUpdate(id) } } }
                } label: {
                    if busy { Label("Updating…", systemImage: "arrow.triangle.2.circlepath") }
                    else {
                        Label((pending.count == 1 ? L10n.text("Update download") : L10n.format("Update %lld downloads", pending.count)) + " · " + bytes(size), systemImage: "arrow.triangle.2.circlepath")
                    }
                }.buttonStyle(MonoButtonStyle()).disabled(busy)
            }
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
            asset.season == season && !queue.items.contains { $0.server == store.libraryKey && $0.assetID == asset.id && $0.state != .removed && $0.state != .removing && $0.state != .failed }
        }
    }
    /// This device's downloads for the season, finished ones included.
    private var items: [DownloadStorage.Item] {
        queue.items.filter { $0.server == store.libraryKey && $0.titleID == title.id && $0.season == season && $0.state != .removed && $0.state != .removing }
    }
    var body: some View {
        // Whatever the season's state, finished episodes can be offered their update.
        VStack(alignment: .leading, spacing: 8) {
            state
            DownloadUpdateOffer(ids: items.filter { $0.state == .ready }.map(\.id))
        }.task(id: title.id + (store.connected ? "|connected" : "")) { await queue.checkForUpdates(title: title) }
    }
    @ViewBuilder private var state: some View {
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
