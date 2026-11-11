import SwiftUI
import AVKit
import OSLog

@MainActor
private enum PlaybackSpeeds {
    static let rates: [Float] = [0.5, 0.75, 1, 1.25, 1.5, 1.75, 2]
    static var options: [AVPlaybackSpeed] {
        zip(rates, ["0.5×", "0.75×", "1×", "1.25×", "1.5×", "1.75×", "2×"])
            .map { AVPlaybackSpeed(rate: $0.0, localizedName: $0.1) }
    }
    static var preferred: Float {
        let rate = UserDefaults.standard.float(forKey: "playback.speed")
        return rates.contains(rate) ? rate : 1
    }
    static func remember(_ rate: Float) {
        guard rates.contains(rate) else { return }
        UserDefaults.standard.set(rate, forKey: "playback.speed")
    }
}

@MainActor
func skip(_ player: AVPlayer, seconds: Double) {
    let current = player.currentTime().seconds
    guard current.isFinite else { return }
    let duration = player.currentItem?.duration.seconds ?? .infinity
    let end = duration.isFinite ? max(0, duration - 0.1) : .infinity
    player.seek(to: CMTime(seconds: min(end, max(0, current + seconds)), preferredTimescale: 600), toleranceBefore: .zero, toleranceAfter: .zero)
}

@MainActor
final class PlaybackModel: ObservableObject {
    let player: AVPlayer
    @Published var error: String?
    @Published var title: String
    @Published var endMessage: String?
    /// A movie or special feature played to its end: nothing follows, so the player closes itself.
    @Published var closeRequested = false
    @Published var preparingNext = false
    @Published var canAdvanceEpisode = false
    private var progressTicks = 0
    private var nextEpisodeTask: Task<Void, Never>?
    private var subtitleController: SubtitlePlaybackController?
    private var observation: NSKeyValueObservation?
    private var timeObserver: Any?
    private var rateObservation: NSKeyValueObservation?
    private var speedObservation: NSKeyValueObservation?
    private var profileObserver: NSObjectProtocol?
    private var endObserver: NSObjectProtocol?
    private var sequence = 0
    private var starting = false
    private var resumed = false
    private var finished = false
    private var playback: Playback
    private let store: Store
    private let profileAtStart: String
    init(playback: Playback, store: Store) {
        self.playback = playback
        self.store = store
        self.profileAtStart = store.activeProfile?.id ?? "profile-jon"
        self.title = playback.name
        player = AVPlayer()
        player.defaultRate = PlaybackSpeeds.preferred
        load(playback)
        profileObserver = NotificationCenter.default.addObserver(forName: .torrbabyProfileWillChange, object: store, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.stop()
                self?.endMessage = "User switched"
            }
        }
        #if os(iOS) || os(tvOS)
        try? AVAudioSession.sharedInstance().setCategory(.playback, mode: .moviePlayback)
        try? AVAudioSession.sharedInstance().setActive(true)
        #endif
        timeObserver = player.addPeriodicTimeObserver(forInterval: CMTime(seconds: 1, preferredTimescale: 600), queue: .main) { [weak self] _ in
            Task { @MainActor [weak self] in
                guard let self else { return }
                self.updateNextEpisodeAvailability()
                self.progressTicks += 1
                if self.progressTicks % 5 == 0 { self.report() }
            }
        }
        rateObservation = player.observe(\.rate, options: [.new]) { [weak self] _, _ in
            Task { @MainActor [weak self] in self?.report() }
        }
        speedObservation = player.observe(\.defaultRate, options: [.new]) { player, _ in
            let rate = player.defaultRate
            Task { @MainActor in PlaybackSpeeds.remember(rate) }
        }
    }
    private func load(_ next: Playback) {
        if let endObserver { NotificationCenter.default.removeObserver(endObserver) }
        observation = nil
        subtitleController?.stop(); subtitleController = nil
        playback = next; title = next.name; error = nil; endMessage = nil
        sequence = 0; starting = false; resumed = false
        let url = playback.url
        let title = playback.name
        // Also cover restored Mac playback windows and connection loss between
        // choosing a title and constructing the native player.
        guard url.isFileURL ? FileManager.default.fileExists(atPath: url.path) : store.canStream else {
            player.replaceCurrentItem(with: nil)
            error = "This title isn’t downloaded. Reconnect to your library to play it."
            return
        }
        let item = AVPlayerItem(url: url)
        let playbackID = playback.id
        #if os(iOS) || os(tvOS)
        let metadata = AVMutableMetadataItem()
        metadata.identifier = .commonIdentifierTitle
        metadata.value = title as NSString
        metadata.extendedLanguageTag = "und"
        item.externalMetadata = [metadata]
        #endif
        player.replaceCurrentItem(with: item)
        endObserver = NotificationCenter.default.addObserver(forName: .AVPlayerItemDidPlayToEndTime, object: item, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated {
                guard self?.playback.id == playbackID else { return }
                self?.advanceToNextEpisode(reachedEnd: true)
            }
        }
        observation = item.observe(\.status, options: [.new, .initial]) { [weak self] item, _ in
            if item.status == .readyToPlay {
                Task { @MainActor [weak self] in
                    guard let self, self.player.currentItem === item else { return }
                    if self.subtitleController == nil {
                        self.subtitleController = SubtitlePlaybackController(item: item, store: self.store)
                    }
                    self.start()
                }
            }
            if item.status == .failed {
                let message = item.error?.localizedDescription ?? "Apple's player cannot open this file."
                Task { @MainActor [weak self] in self?.error = message }
            }
        }
    }
    func updateNextEpisodeAvailability() {
        let duration = player.currentItem?.duration.seconds ?? 0
        let position = player.currentTime().seconds
        // Runs every second: only look the next episode up near the end, and only publish a change.
        var available = false
        if !finished && !preparingNext && resumed && duration.isFinite && duration > 0 && position.isFinite && position >= duration * 0.9 {
            #if os(tvOS)
            if let titleID = playback.libraryTitleID, let assetID = playback.libraryAssetID,
               let title = store.library?.titles.first(where: { $0.id == titleID }) {
                available = title.nextEpisode(after: assetID)?.complete == true
            }
            #else
            available = DownloadQueue.shared.hasNextEpisode(after: playback)
            #endif
        }
        if canAdvanceEpisode != available { canAdvanceEpisode = available }
    }
    func advanceToNextEpisode(reachedEnd: Bool = false) {
        guard !finished, !preparingNext, resumed, reachedEnd || canAdvanceEpisode else { return }
        let playbackID = playback.id
        nextEpisodeTask = Task { @MainActor [weak self] in
            guard let self, !self.finished, !self.preparingNext, self.resumed, self.playback.id == playbackID else { return }
            self.canAdvanceEpisode = false
                // Choosing Next Episode during credits is an explicit completion,
                // including when paused. Use the same watched event as natural EOF.
                let progressSaved = self.report(ended: true, reachedEnd: true)
                self.player.pause()
                #if os(tvOS)
                await self.advanceTVEpisode(reachedEnd: reachedEnd)
                #else
                let queue = DownloadQueue.shared
                let outgoing = self.playback
                let episode = queue.isEpisode(outgoing)
                if outgoing.localDownloadID != nil || episode {
                    self.resumed = false
                    self.preparingNext = episode
                    defer { self.preparingNext = false }
                    do {
                        // Keep AVKit attached to its item while the next stream prepares.
                        // Clearing it here can dismiss mobile playback before replacement.
                        if episode, let next = try await queue.nextPlayback(after: outgoing) {
                            guard !self.finished, !Task.isCancelled, self.playback.id == outgoing.id else { return }
                            self.load(next)
                        } else if !self.finished {
                            if episode {
                                self.endMessage = queue.autoplayStopMessage(after: outgoing)
                                self.player.replaceCurrentItem(with: nil)
                            } else { self.closeRequested = true }
                        }
                        if progressSaved, let id = outgoing.localDownloadID { queue.finished(id) }
                    } catch is CancellationError {
                        return
                    } catch {
                        if !self.finished { self.error = "Couldn’t start the next episode. " + error.localizedDescription }
                    }
                } else if reachedEnd { self.closeRequested = true }
                #endif
        }
    }
    #if os(tvOS)
    private func advanceTVEpisode(reachedEnd: Bool) async {
        guard let titleID = playback.libraryTitleID, let assetID = playback.libraryAssetID,
              let title = store.library?.titles.first(where: { $0.id == titleID }), title.kind == "tv",
              let current = title.assets.first(where: { $0.id == assetID }), current.episode != nil, current.role != "extra" else {
            if reachedEnd { closeRequested = true }
            return
        }
        resumed = false
        guard let next = title.nextEpisode(after: assetID), next.complete else {
            endMessage = title.autoplayStopMessage(after: assetID)
            return
        }
        preparingNext = true
        defer { preparingNext = false }
        let server = store.server
        do {
            await store.flushProgress()
            try Task.checkCancellation()
            try await store.registerClient()
            let link = try await store.mediaPlaybackLink(asset: next, titleID: titleID)
            guard !finished, !Task.isCancelled, store.server == server else { return }
            load(Playback(sessionId: link.sessionId, resumePosition: link.resumePosition ?? 0,
                          url: try store.endpoint(link.path), name: next.name ?? title.title,
                          libraryAssetID: next.id, libraryTitleID: titleID))
        } catch {
            if !finished, !Task.isCancelled { endMessage = "Couldn’t start \(EpisodeNumber(season: next.season ?? 0, episode: next.episode ?? 0).label)" }
        }
    }
    #endif
    private func start() {
        guard !starting, !finished else { return }
        starting = true
        if playback.resumePosition > 0 {
            player.seek(to: CMTime(seconds: playback.resumePosition, preferredTimescale: 600), toleranceBefore: .zero, toleranceAfter: .zero) { [weak self] _ in
                Task { @MainActor [weak self] in
                    guard let self, !self.finished else { return }
                    self.resumed = true
                    self.player.play()
                }
            }
        } else { resumed = true; player.play() }
    }
    @discardableResult
    func report(ended: Bool = false, reachedEnd: Bool = false) -> Bool {
        guard resumed, !finished else { return false }
        let duration = player.currentItem?.duration.seconds ?? 0
        let position = player.currentTime().seconds
        guard duration.isFinite, duration > 0, position.isFinite else { return false }
        #if !os(tvOS)
        if let id = playback.localDownloadID {
            return DownloadQueue.shared.report(id, profile: profileAtStart, position: position, duration: duration, reachedEnd: reachedEnd, watchedEligible: player.rate > 0)
        }
        #endif
        guard let session = playback.sessionId else { return false }
        sequence += 1
        store.reportProgress(sessionId: session, sequence: sequence, position: position, duration: duration, ended: ended, watchedEligible: player.rate > 0, reachedEnd: reachedEnd)
        return true
    }
    func stop() {
        guard !finished else { return }
        if let profileObserver { NotificationCenter.default.removeObserver(profileObserver); self.profileObserver = nil }
        nextEpisodeTask?.cancel(); nextEpisodeTask = nil
        subtitleController?.stop(); subtitleController = nil
        player.pause()
        let saved = report(ended: true)
        if !saved, let session = playback.sessionId {
            Task { struct Ended: Decodable { let accepted: Bool }
                let _: Ended? = try? await store.request("POST", "/v1/playback/end", body: ["session_id": session])
            }
        }
        finished = true
        player.replaceCurrentItem(with: nil)
        if let timeObserver { player.removeTimeObserver(timeObserver); self.timeObserver = nil }
        rateObservation = nil
        speedObservation = nil
        observation = nil
        if let endObserver { NotificationCenter.default.removeObserver(endObserver); self.endObserver = nil }
    }
}

#if os(macOS)
struct NativePlayer: NSViewRepresentable {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    let title: String
    let player: AVPlayer
    let close: () -> Void
    var canAdvance = false
    var advance: () -> Void = {}
    class Coordinator: NSObject {
        var advance: () -> Void = {}
        @objc func nextEpisode(_ sender: NSMenuItem) { advance() }
        var monitor: Any?
        var mouseMonitor: Any?
        var hideWork: DispatchWorkItem?
        weak var window: NSWindow?
        func header(_ visible: Bool) {
            guard let window, !window.styleMask.contains(.fullScreen) else { return }
            window.titleVisibility = visible ? .visible : .hidden
            for button in [NSWindow.ButtonType.closeButton, .miniaturizeButton, .zoomButton] {
                window.standardWindowButton(button)?.isHidden = !visible
            }
        }
        func configure(_ window: NSWindow, title: String) {
            window.title = title
            guard self.window !== window else { return }
            self.window = window
            window.titlebarAppearsTransparent = true
            window.styleMask.insert(.fullSizeContentView)
            window.acceptsMouseMovedEvents = true
            header(true)
            let work = DispatchWorkItem { [weak self] in self?.header(false) }
            hideWork = work
            DispatchQueue.main.asyncAfter(deadline: .now() + 3, execute: work)
            mouseMonitor = NSEvent.addLocalMonitorForEvents(matching: [.mouseMoved, .leftMouseDown, .keyDown]) { [weak self] event in
                guard let self, event.window === self.window, let window = self.window else { return event }
                if event.type == .mouseMoved || event.type == .leftMouseDown {
                    self.hideWork?.cancel()
                    self.header(event.locationInWindow.y >= window.frame.height - 52)
                }
                return event
            }
        }
        func stop() {
            hideWork?.cancel()
            if let monitor { NSEvent.removeMonitor(monitor) }; monitor = nil
            if let mouseMonitor { NSEvent.removeMonitor(mouseMonitor) }; mouseMonitor = nil
        }
        deinit { stop() }
    }
    func makeCoordinator() -> Coordinator { Coordinator() }
    func makeNSView(context: Context) -> AVPlayerView {
        let view = AVPlayerView()
        view.player = player
        view.controlsStyle = .floating
        view.speeds = PlaybackSpeeds.options
        view.showsFullScreenToggleButton = true
        view.allowsPictureInPicturePlayback = true
        DispatchQueue.main.async { [weak view, weak coordinator = context.coordinator] in
            if let window = view?.window { coordinator?.configure(window, title: title) }
        }
        context.coordinator.monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak view, weak player] event in
            guard let view, let window = view.window, window.isKeyWindow, let player,
                  event.modifierFlags.intersection([.command, .option, .control]).isEmpty else { return event }
            if event.keyCode == 123 { skip(player, seconds: -15); return nil }
            if event.keyCode == 124 { skip(player, seconds: 15); return nil }
            if event.keyCode == 53 && !window.styleMask.contains(.fullScreen) { close(); return nil }
            return event
        }
        return view
    }
    func updateNSView(_ view: AVPlayerView, context: Context) {
        view.player = player
        context.coordinator.advance = advance
        if canAdvance {
            if view.actionPopUpButtonMenu == nil {
                let menu = NSMenu()
                let item = NSMenuItem(title: L10n.text("Next episode"), action: #selector(Coordinator.nextEpisode(_:)), keyEquivalent: "")
                item.target = context.coordinator
                item.image = NSImage(systemSymbolName: "forward.end.fill", accessibilityDescription: "Next episode")
                menu.addItem(item)
                view.actionPopUpButtonMenu = menu
            }
        } else { view.actionPopUpButtonMenu = nil }
        if let window = view.window { context.coordinator.configure(window, title: title) }
    }
    static func dismantleNSView(_ view: AVPlayerView, coordinator: Coordinator) { coordinator.stop(); view.player = nil }
}
#else
class KeyboardPlayerController: AVPlayerViewController {
    var close: (() -> Void)?
    override var canBecomeFirstResponder: Bool { true }
    override func viewDidAppear(_ animated: Bool) { super.viewDidAppear(animated); becomeFirstResponder() }
    override var keyCommands: [UIKeyCommand]? {
        let commands = [
            UIKeyCommand(input: " ", modifierFlags: [], action: #selector(togglePlayback)),
            UIKeyCommand(input: UIKeyCommand.inputLeftArrow, modifierFlags: [], action: #selector(back)),
            UIKeyCommand(input: UIKeyCommand.inputRightArrow, modifierFlags: [], action: #selector(forward)),
            UIKeyCommand(input: UIKeyCommand.inputEscape, modifierFlags: [], action: #selector(exitPlayer))
        ]
        let titles = ["Play/Pause", "Skip back 15 seconds", "Skip forward 15 seconds", "Close player"]
        for (command, title) in zip(commands, titles) {
            command.wantsPriorityOverSystemBehavior = true
            command.discoverabilityTitle = title
        }
        // Preserve AVKit's other commands instead of replacing its entire list.
        let inherited = (super.keyCommands ?? []).filter { original in
            !commands.contains { $0.input == original.input && $0.modifierFlags == original.modifierFlags }
        }
        return commands + inherited
    }
    @objc private func togglePlayback() {
        guard let player else { return }
        if player.timeControlStatus == .paused { player.play() } else { player.pause() }
    }
    @objc private func back() { if let player { skip(player, seconds: -15) } }
    @objc private func forward() { if let player { skip(player, seconds: 15) } }
    @objc private func exitPlayer() { close?() }
}
struct NativePlayer: UIViewControllerRepresentable {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    let player: AVPlayer
    let close: () -> Void
    var canAdvance = false
    var advance: () -> Void = {}
    func makeUIViewController(context: Context) -> KeyboardPlayerController {
        let view = KeyboardPlayerController()
        view.player = player
        view.close = close
        view.showsPlaybackControls = true
        view.speeds = PlaybackSpeeds.options
        view.allowsPictureInPicturePlayback = true
        return view
    }
    func updateUIViewController(_ view: KeyboardPlayerController, context: Context) {
        view.player = player; view.close = close
        #if os(tvOS)
        view.transportBarCustomMenuItems = canAdvance
            ? [UIAction(title: L10n.text("Next episode"), image: UIImage(systemName: "forward.end.fill")) { _ in advance() }] : []
        #endif
    }
}
#endif

struct PlayerScreen: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    let playback: Playback
    @StateObject private var model: PlaybackModel
    @Environment(\.dismiss) private var dismiss
    @Environment(\.scenePhase) private var scenePhase
    init(playback: Playback, store: Store) {
        self.playback = playback
        _model = StateObject(wrappedValue: PlaybackModel(playback: playback, store: store))
    }
    var body: some View {
        Group {
            if model.preparingNext {
                ProgressView("Next episode…").tint(.white)
            } else if let error = model.error {
                ContentUnavailableView {
                    Label("This file couldn’t play", systemImage: "play.slash")
                } description: {
                    Text(error)
                } actions: {
                    Button("Close") { dismiss() }
                }
            } else if let message = model.endMessage {
                ContentUnavailableView { Label(message, systemImage: "checkmark.circle") } actions: {
                    Button("Close") { dismiss() }
                }
            } else {
                #if os(macOS)
                NativePlayer(title: model.title, player: model.player, close: { dismiss() }, canAdvance: model.canAdvanceEpisode, advance: { model.advanceToNextEpisode() })
                #else
                NativePlayer(player: model.player, close: { dismiss() }, canAdvance: model.canAdvanceEpisode, advance: { model.advanceToNextEpisode() })
                #endif
            }
        }
        .background(.black)
        #if !os(tvOS)
        .presentationBackground(.black)
        #endif
        #if os(macOS)
        .frame(minWidth: 480, minHeight: 270)
        .ignoresSafeArea()
        .onExitCommand { dismiss() }
        #else
        .ignoresSafeArea()
        #endif
        .onChange(of: scenePhase) { _, _ in model.report() }
        .onChange(of: model.closeRequested) { _, close in if close { dismiss() } }
        .onDisappear { model.stop() }
    }
}


@MainActor
private enum SubtitlePreferenceCache {
    static func key(_ store: Store) -> String { "subtitle-defaults:" + store.server + ":" + store.clientId }
    static func load(_ store: Store) -> SubtitlePreferences? {
        UserDefaults.standard.data(forKey: key(store)).flatMap { try? JSONDecoder().decode(SubtitlePreferences.self, from: $0) }
    }
    static func save(_ value: SubtitlePreferences, store: Store, pending: Bool) {
        if let data = try? JSONEncoder().encode(value) { UserDefaults.standard.set(data, forKey: key(store)) }
        UserDefaults.standard.set(pending, forKey: key(store) + ":pending")
    }
}

/// Native subtitle rendering and native selection UI remain owned by AVKit.
@MainActor
final class SubtitlePlaybackController {
    private var task: Task<Void, Never>?
    private var observer: NSObjectProtocol?
    private var lastOption: AVMediaSelectionOption?
    private var preferences = SubtitlePreferences()
    private var stopped = false
    private let logger = Logger(subsystem: "org.torrbaby", category: "Subtitles")
    init(item: AVPlayerItem, store: Store) {
        task = Task { [weak self, weak item] in
            guard let self, let item else { return }
            let path = store.deviceSubtitlePath
            do {
                let cached = SubtitlePreferenceCache.load(store)
                let local = (item.asset as? AVURLAsset)?.url.isFileURL == true
                var p = cached ?? SubtitlePreferences()
                if !local {
                    if let cached, UserDefaults.standard.bool(forKey: SubtitlePreferenceCache.key(store) + ":pending") {
                        if (try? await store.send("PUT", path, body: cached.body)) != nil {
                            SubtitlePreferenceCache.save(cached, store: store, pending: false)
                        }
                    } else if cached == nil, let response: SubtitlePreferenceResponse = try? await store.request("GET", path, timeout: 5) {
                        p = response.effective
                        SubtitlePreferenceCache.save(p, store: store, pending: false)
                    }
                }
                guard !Task.isCancelled, !self.stopped else { return }
                self.preferences = p
                let fonts = ["default": kCMTextMarkupGenericFontName_Default, "serif": kCMTextMarkupGenericFontName_Serif,
                             "sans-serif": kCMTextMarkupGenericFontName_SansSerif, "monospace": kCMTextMarkupGenericFontName_Monospace]
                let attributes: [String: Any] = [
                    kCMTextMarkupAttribute_RelativeFontSize as String: p.sizePercent,
                    kCMTextMarkupAttribute_GenericFontFamilyName as String: fonts[p.font] ?? kCMTextMarkupGenericFontName_Default,
                    kCMTextMarkupAttribute_ForegroundColorARGB as String: [Double(p.textOpacity) / 100, 1, 1, 1],
                    kCMTextMarkupAttribute_BackgroundColorARGB as String: [Double(p.backgroundOpacity) / 100, 0, 0, 0]
                ]
                if let rule = AVTextStyleRule(textMarkupAttributes: attributes) { item.textStyleRules = [rule] }
                guard let group = try await item.asset.loadMediaSelectionGroup(for: .legible), !self.stopped else { return }
                // A forced-only track can contain no dialogue at all. Prefer full
                // subtitles when the user explicitly enables subtitles in Settings.
                let fullOptions = group.options.filter { !$0.hasMediaCharacteristic(.containsOnlyForcedSubtitles) }
                let options = fullOptions.isEmpty ? group.options : fullOptions
                let fallback = options.first { $0 == group.defaultOption } ?? options.first
                let option: AVMediaSelectionOption?
                if !p.enabled { option = nil }
                else if p.language == "original" { option = fallback }
                else {
                    option = AVMediaSelectionGroup.mediaSelectionOptions(from: options, with: Locale(identifier: p.language)).first
                        ?? fallback
                }
                self.logger.info("Applying subtitle default enabled=\(p.enabled) language=\(p.language, privacy: .public) options=\(options.count) selected=\(option?.extendedLanguageTag ?? "off", privacy: .public)")
                item.select(option, in: group)
                // Selection is asynchronous. Do not interpret its intermediate nil
                // state as the user switching subtitles off and persist that state.
                // Confirm the requested selection before observing user changes.
                for _ in 0..<40 {
                    guard !Task.isCancelled, !self.stopped else { return }
                    if item.currentMediaSelection.selectedMediaOption(in: group) == option { break }
                    try await Task.sleep(for: .milliseconds(50))
                }
                if item.currentMediaSelection.selectedMediaOption(in: group) == option {
                    self.logger.info("Subtitle default confirmed")
                } else {
                    self.logger.error("Subtitle default did not settle; preserving saved preference")
                }
                // Even on a failed selection, keep observing later explicit choices.
                self.lastOption = item.currentMediaSelection.selectedMediaOption(in: group)
                self.observer = NotificationCenter.default.addObserver(forName: AVPlayerItem.mediaSelectionDidChangeNotification, object: item, queue: .main) { [weak self, weak item] _ in
                    Task { @MainActor [weak self, weak item] in
                        guard let self, let item, !self.stopped else { return }
                        let selected = item.currentMediaSelection.selectedMediaOption(in: group)
                        guard selected != self.lastOption else { return }
                        self.lastOption = selected
                        self.preferences.enabled = selected != nil
                        if let selected {
                            let language = selected.extendedLanguageTag ?? selected.locale?.identifier ?? ""
                            if language.hasPrefix("zh") { self.preferences.language = "zh-Hans" }
                            else if language.hasPrefix("en") { self.preferences.language = "en" }
                            else { self.preferences.language = "original" }
                        }
                        SubtitlePreferenceCache.save(self.preferences, store: store, pending: true)
                        if store.connected {
                            do {
                                try await store.send("PUT", store.deviceSubtitlePath, body: self.preferences.body)
                                SubtitlePreferenceCache.save(self.preferences, store: store, pending: false)
                            } catch { /* Keep the device preference for the next connection. */ }
                        }
                    }
                }
            } catch {
                self.logger.error("Could not apply subtitle preference: \(error.localizedDescription, privacy: .public)")
            }
        }
    }
    func stop() {
        stopped = true; task?.cancel(); task = nil
        if let observer { NotificationCenter.default.removeObserver(observer); self.observer = nil }
    }
}

struct SubtitlePreferencesView: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) private var dismiss
    @State private var preferences = SubtitlePreferences()
    @State private var loaded = false
    @State private var saving = false
    @State private var error: String?
    private var path: String { store.deviceSubtitlePath }
    var body: some View {
        NavigationStack {
            Form {
                if let error { Text(L10n.text(error)).foregroundStyle(.orange) }
                SwiftUI.Section {
                    subtitleChoice("Off", value: "off")
                    subtitleChoice("English", value: "en")
                    subtitleChoice("Chinese (Simplified)", value: "zh-Hans")
                    NavigationLink("Style") {
                        Form {
                            Picker("Font", selection: $preferences.font) {
                                Text("Default").tag("default")
                                Text("Sans serif").tag("sans-serif")
                                Text("Serif").tag("serif")
                                Text("Monospace").tag("monospace")
                            }
                            Picker("Size", selection: $preferences.sizePercent) {
                                ForEach(Array(stride(from: 50, through: 200, by: 10)), id: \.self) { Text("\($0)%").tag($0) }
                            }
                            Picker("Text opacity", selection: $preferences.textOpacity) {
                                ForEach(Array(stride(from: 0, through: 100, by: 10)), id: \.self) { Text("\($0)%").tag($0) }
                            }
                            Picker("Background opacity", selection: $preferences.backgroundOpacity) {
                                ForEach(Array(stride(from: 0, through: 100, by: 10)), id: \.self) { Text("\($0)%").tag($0) }
                            }
                        }.navigationTitle("Subtitle style")
                    }
                }.disabled(!loaded || saving)
            }.navigationTitle("Subtitles on this device")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) { Button("Save") { save() }.disabled(!loaded || saving) }
            }
        }.task {
            if let cached = SubtitlePreferenceCache.load(store) {
                preferences = cached; loaded = true
                return
            }
            do {
                let response: SubtitlePreferenceResponse = try await store.request("GET", path)
                preferences = response.defaults
                SubtitlePreferenceCache.save(preferences, store: store, pending: false)
                loaded = true
            } catch { self.error = error.localizedDescription }
        }
        #if os(macOS)
        .frame(width: 500, height: 470)
        #endif
    }
    private func subtitleChoice(_ title: String, value: String) -> some View {
        let selected = preferences.enabled ? preferences.language : "off"
        return Button {
            preferences.enabled = value != "off"
            if value != "off" { preferences.language = value }
        } label: {
            HStack {
                Text(LocalizedStringKey(title)).foregroundStyle(.primary)
                Spacer()
                if selected == value { Image(systemName: "checkmark").foregroundStyle(.tint) }
            }.contentShape(Rectangle())
        }
        .accessibilityAddTraits(selected == value ? .isSelected : [])
    }

    private func save() {
        saving = true; error = nil
        Task {
            defer { saving = false }
            do {
                try await store.send("PUT", path, body: preferences.body)
                SubtitlePreferenceCache.save(preferences, store: store, pending: false)
                dismiss()
            } catch { self.error = error.localizedDescription }
        }
    }
}
