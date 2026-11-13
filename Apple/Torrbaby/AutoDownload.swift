#if os(iOS)
import BackgroundTasks
import SwiftUI
import UserNotifications

/// New episodes of chosen shows, kept on this iPhone or iPad without opening the app.
///
/// A show is chosen on its page. The next few unwatched episodes of each chosen show are
/// queued through the ordinary download queue, whose transfers iOS carries on while the
/// app is closed and which never uses cellular unless the member has allowed it. iOS wakes
/// the app to look for new episodes a few times a day and, on charge, overnight; the
/// server looks for new episodes around midday and in the evening, so the overnight wake
/// finds them. All automatic downloads together stay under one size limit.
@MainActor
enum AutoDownload {
    static let refreshID = "com.jonthomason.torrbaby.episodes.refresh"
    static let overnightID = "com.jonthomason.torrbaby.episodes.overnight"
    private static let followedKey = "autoDownload.titles", keepKey = "autoDownload.keep", limitKey = "autoDownload.limitBytes", queuedKey = "autoDownload.queued"
    /// The app's store, when it has one; a background launch may have to make its own.
    static weak var store: Store?
    private static var fallback: Store?
    private static var lastRun = Date.distantPast
    /// One pass at a time: a pass can take minutes while the server prepares files.
    private static var running = false

    static var followed: Set<String> {
        get { Set(UserDefaults.standard.stringArray(forKey: followedKey) ?? []) }
        set { UserDefaults.standard.set(Array(newValue).sorted(), forKey: followedKey) }
    }
    /// How many unwatched episodes of each show to hold.
    static var keep: Int {
        get { let saved = UserDefaults.standard.integer(forKey: keepKey); return saved > 0 ? saved : 3 }
        set { UserDefaults.standard.set(min(20, max(1, newValue)), forKey: keepKey) }
    }
    /// The most space all automatic downloads may take together.
    static var limitBytes: Int64 {
        get { let saved = (UserDefaults.standard.object(forKey: limitKey) as? NSNumber)?.int64Value ?? 0; return saved > 0 ? saved : 20_000_000_000 }
        set { UserDefaults.standard.set(NSNumber(value: max(1_000_000_000, newValue)), forKey: limitKey) }
    }
    /// Downloads this feature queued, so their space and notices can be told from the member's own.
    private static var queued: Set<String> {
        get { Set(UserDefaults.standard.stringArray(forKey: queuedKey) ?? []) }
        set { UserDefaults.standard.set(Array(newValue), forKey: queuedKey) }
    }
    static func isFollowed(_ title: String) -> Bool { followed.contains(title) }
    static func setFollowed(_ title: String, _ on: Bool) {
        var all = followed
        if on { all.insert(title) } else { all.remove(title) }
        followed = all
        if on { schedule(); if let store { Task { await run(store: store, force: true) } } }
    }
    /// Space the automatic downloads still on the device take.
    static var usedBytes: Int64 {
        let mine = queued
        return DownloadQueue.shared.items.filter { mine.contains($0.id.uuidString) && $0.state != .removed && $0.state != .removing }.reduce(0) { $0 + $1.size }
    }

    /// Queue the next unwatched episodes of each chosen show that are not on the device
    /// yet, in order, until the size limit is reached. Returns how many were queued.
    @discardableResult
    static func run(store: Store, force: Bool = false) async -> Int {
        guard !followed.isEmpty, store.connected, let library = store.library else { return 0 }
        // The library refreshes every few seconds while the app is open; this need not.
        guard force || Date().timeIntervalSince(lastRun) > 120, !running else { return 0 }
        lastRun = Date(); running = true
        defer { running = false }
        let queue = DownloadQueue.shared
        // Forget downloads that have since been removed, so the list does not grow without end.
        let present = Set(queue.items.filter { $0.state != .removed }.map(\.id.uuidString))
        queued = queued.intersection(present)
        var added = 0
        for title in library.titles where title.kind == "tv" && followed.contains(title.id) {
            var seen = Set<EpisodeNumber>()
            let next = title.playable.filter { asset in
                guard !asset.watched, asset.offline != true, let season = asset.season, let episode = asset.episode, season > 0 else { return false }
                return seen.insert(EpisodeNumber(season: season, episode: episode)).inserted
            }.prefix(keep)
            for asset in next where !queue.items.contains(where: { $0.server == store.libraryKey && $0.assetID == asset.id && $0.state != .removed }) {
                guard usedBytes < limitBytes else { return added }
                await queue.enqueue(asset, title: title)
                guard let item = queue.items.first(where: { $0.server == store.libraryKey && $0.assetID == asset.id && $0.state != .removed }) else { continue }
                queued.insert(item.id.uuidString)
                // Its size is only known once queued: one that would break the limit is taken back.
                if usedBytes > limitBytes { queue.remove(item.id); queued.remove(item.id.uuidString); return added }
                added += 1
            }
        }
        return added
    }
    /// Tell the member an automatically queued episode has arrived.
    static func arrived(_ item: DownloadStorage.Item) {
        guard queued.contains(item.id.uuidString) else { return }
        Task {
            let center = UNUserNotificationCenter.current()
            let settings = await center.notificationSettings()
            guard settings.authorizationStatus == .authorized || settings.authorizationStatus == .provisional else { return }
            let content = UNMutableNotificationContent()
            content.title = L10n.text("New episode downloaded")
            content.body = [item.title, item.season.flatMap { season in item.episode.map { L10n.format("Season %d Ep %d", season, $0) } } ?? ""].filter { !$0.isEmpty }.joined(separator: " · ")
            try? await center.add(UNNotificationRequest(identifier: "auto-" + item.id.uuidString, content: content, trigger: nil))
        }
    }

    // MARK: Waking without the app being opened
    /// Must run before the app finishes launching.
    nonisolated static func register() {
        for id in [refreshID, overnightID] {
            BGTaskScheduler.shared.register(forTaskWithIdentifier: id, using: nil) { task in Task { @MainActor in handle(task) } }
        }
    }
    /// Ask iOS for the next wakes: a short one when it sees fit, and a longer one on
    /// charge with a network, which in practice is overnight. iOS chooses the moments.
    static func schedule() {
        guard !followed.isEmpty else { return }
        let refresh = BGAppRefreshTaskRequest(identifier: refreshID)
        refresh.earliestBeginDate = Date(timeIntervalSinceNow: 2 * 3600)
        try? BGTaskScheduler.shared.submit(refresh)
        let overnight = BGProcessingTaskRequest(identifier: overnightID)
        overnight.requiresNetworkConnectivity = true
        overnight.requiresExternalPower = true
        overnight.earliestBeginDate = Date(timeIntervalSinceNow: 3 * 3600)
        try? BGTaskScheduler.shared.submit(overnight)
    }
    private static func handle(_ task: BGTask) {
        schedule()
        let work = Task { @MainActor in
            // A background launch may not have built the interface, and with it the store.
            if store == nil { try? await Task.sleep(for: .seconds(2)) }
            if store == nil { fallback = Store() }
            guard let store = store ?? fallback else { task.setTaskCompleted(success: false); return }
            await store.refresh()
            guard !Task.isCancelled else { return }
            await run(store: store, force: true)
            task.setTaskCompleted(success: store.connected)
        }
        task.expirationHandler = { work.cancel(); task.setTaskCompleted(success: false) }
    }
}

/// The switch on a show's page.
struct AutoDownloadToggle: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    let title: LibraryTitle
    @State private var on = false
    var body: some View {
        Toggle(isOn: Binding(get: { on }, set: { value in on = value; AutoDownload.setFollowed(title.id, value) })) {
            Label("Keep new episodes on this device", systemImage: "arrow.down.circle")
        }
        .font(.subheadline).tint(.green)
        .onAppear { on = AutoDownload.isFollowed(title.id) }
        .onChange(of: title.id) { _, id in on = AutoDownload.isFollowed(id) }
    }
}

/// The limits, in the download settings.
struct AutoDownloadSettings: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @ObservedObject private var queue = DownloadQueue.shared
    @State private var keep = AutoDownload.keep
    @State private var limit = Double(AutoDownload.limitBytes) / 1_000_000_000
    var body: some View {
        SwiftUI.Section {
            Stepper(value: $keep, in: 1...20) { LabeledContent("Episodes to keep per show", value: "\(keep)") }
                .onChange(of: keep) { _, value in AutoDownload.keep = value }
            LabeledContent("Automatic download limit (GB)") {
                TextField("Automatic download limit (GB)", value: $limit, format: .number).labelsHidden().multilineTextAlignment(.trailing).keyboardType(.decimalPad)
                    .onChange(of: limit) { _, value in if value >= 1 { AutoDownload.limitBytes = Int64(value * 1_000_000_000) } }
            }
            LabeledContent("In use", value: ByteCountFormatter.string(fromByteCount: AutoDownload.usedBytes, countStyle: .file))
        } header: { Text("New episodes") } footer: {
            Text("Shows you turn this on for keep their next unwatched episodes on this device. They arrive by themselves, a few times a day and overnight while charging, and only on Wi‑Fi.")
        }
    }
}
#endif
