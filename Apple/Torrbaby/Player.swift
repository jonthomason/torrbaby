import SwiftUI
import AVKit
import OSLog
#if os(iOS)
import MediaPlayer
#endif

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

/// What the player names: a movie's name, or the show's name above "Season 1 Ep 2 – Episode
/// name" (an episode's number alone when it has no title of its own). iPhone, iPad and Apple TV
/// show it natively above the scrubber; the Mac puts it in the window title.
struct PlayerHeading: Equatable {
    var title: String
    /// The show (or movie) a special feature or episode belongs to, in smaller type above.
    var context: String? = nil
    @MainActor static func make(_ playback: Playback, library: MediaLibrary?) -> PlayerHeading {
        guard let titleID = playback.libraryTitleID,
              let title = library?.titles.first(where: { $0.id == titleID }) else { return PlayerHeading(title: playback.name) }
        guard let asset = playback.libraryAssetID.flatMap({ id in title.assets.first { $0.id == id } }) else { return PlayerHeading(title: title.title) }
        if asset.role == "extra" {
            guard let name = asset.ownName, name != title.title else { return PlayerHeading(title: title.title) }
            return PlayerHeading(title: name, context: title.title)
        }
        guard title.kind == "tv" else { return PlayerHeading(title: title.title) }
        let number: String?
        if let episode = asset.episode {
            number = asset.season.map { L10n.format("Season %d Ep %d", $0, episode) } ?? L10n.format("Ep %d", episode)
        } else { number = nil }
        let line = [number, asset.ownName].compactMap { $0 }.joined(separator: " – ")
        return line.isEmpty ? PlayerHeading(title: title.title) : PlayerHeading(title: line, context: title.title)
    }
    /// One line, for a window title or the system's Now Playing.
    var joined: String { [context, title].compactMap { $0 }.joined(separator: " · ") }
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
    /// The native players' title (the Mac's window title); follows the next episode.
    private var heading: PlayerHeading
    @Published var endMessage: String?
    /// A movie or special feature played to its end: nothing follows, so the player closes itself.
    @Published var closeRequested = false
    @Published var preparingNext = false
    @Published var canAdvanceEpisode = false
    /// The on-screen Next Episode prompt, bottom right: shown from the start of
    /// the closing credits until the end, whether or not the controls are up.
    @Published var offerNextEpisode = false
    /// The closing credits Diamond detected for the current file, if any.
    private var credits: EpisodeCredits?
    private var creditsTask: Task<Void, Never>?
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
        self.heading = PlayerHeading(title: playback.name)
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
        playback = next; error = nil; endMessage = nil
        heading = PlayerHeading.make(next, library: store.library)
        title = heading.joined
        sequence = 0; starting = false; resumed = false
        credits = nil; offerNextEpisode = false
        creditsTask?.cancel()
        if let assetID = next.libraryAssetID, store.connected {
            let playbackID = next.id
            creditsTask = Task { @MainActor [weak self] in
                guard let self else { return }
                let analysis: CreditsDetection? = try? await self.store.request("GET", "/v1/library/assets/\(assetID)/credits", timeout: 10)
                guard !Task.isCancelled, self.playback.id == playbackID, let analysis else { return }
                self.credits = analysis.closingCredits
            }
        }
        let url = playback.url
        let heading = self.heading
        // Also cover restored Mac playback windows and connection loss between
        // choosing a title and constructing the native player.
        guard url.isFileURL ? FileManager.default.fileExists(atPath: url.path) : store.canStream else {
            player.replaceCurrentItem(with: nil)
            error = Store.unreachable
            return
        }
        let item = AVPlayerItem(url: url)
        let playbackID = playback.id
        #if os(iOS) || os(tvOS)
        // Shown by AVKit above the scrubber (the show small, over the episode line) and in Now Playing.
        func metadata(_ identifier: AVMetadataIdentifier, _ value: String) -> AVMetadataItem {
            let item = AVMutableMetadataItem()
            item.identifier = identifier
            item.value = value as NSString
            item.extendedLanguageTag = "und"
            return item
        }
        item.externalMetadata = [metadata(.commonIdentifierTitle, heading.title)]
            + (heading.context.map { [metadata(.iTunesMetadataTrackSubTitle, $0)] } ?? [])
        // The title's poster, for Now Playing (the lock screen) when it is in the artwork cache.
        if let titleID = playback.libraryTitleID {
            let key = ArtworkImages.key(server: store.libraryKey, id: titleID, variant: "poster", revision: store.artworkRevision(id: titleID, variant: "poster"))
            Task { @MainActor [weak item] in
                guard let image = await ArtworkImages.prepare(key, variant: "poster"), let data = image.jpegData(compressionQuality: 0.85), let item else { return }
                let artwork = AVMutableMetadataItem()
                artwork.identifier = .commonIdentifierArtwork
                artwork.value = data as NSData
                artwork.dataType = kCMMetadataBaseDataType_JPEG as String
                artwork.extendedLanguageTag = "und"
                item.externalMetadata.append(artwork)
            }
        }
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
                        let info = self.playback.libraryAssetID.flatMap { id in self.store.library?.titles.lazy.flatMap(\.assets).first { $0.id == id }?.mediaInfo }
                        self.subtitleController = SubtitlePlaybackController(item: item, store: self.store, overPictureSubtitles: info?.hasPictureSubtitles == true)
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
        var prompt = false
        if !finished && !preparingNext && resumed && duration.isFinite && duration > 0 && position.isFinite
            && position >= EpisodeCredits.offerStart(credits, duration: duration) {
            #if os(tvOS)
            if let titleID = playback.libraryTitleID, let assetID = playback.libraryAssetID,
               let title = store.library?.titles.first(where: { $0.id == titleID }) {
                available = title.nextEpisode(after: assetID)?.complete == true
            }
            #else
            available = DownloadQueue.shared.hasNextEpisode(after: playback)
            #endif
            prompt = available && EpisodeCredits.prompting(credits, position: position, duration: duration)
        }
        if canAdvanceEpisode != available { canAdvanceEpisode = available }
        if offerNextEpisode != prompt { offerNextEpisode = prompt }
    }
    /// The current file's picture measurements, for fitting it to the screen.
    var video: MediaTrackInfo.Video? {
        guard let id = playback.libraryAssetID else { return nil }
        return store.library?.titles.lazy.flatMap(\.assets).first { $0.id == id }?.mediaInfo?.video
    }
    func advanceToNextEpisode(reachedEnd: Bool = false) {
        guard !finished, !preparingNext, resumed, reachedEnd || canAdvanceEpisode else { return }
        let playbackID = playback.id
        nextEpisodeTask = Task { @MainActor [weak self] in
            guard let self, !self.finished, !self.preparingNext, self.resumed, self.playback.id == playbackID else { return }
            self.canAdvanceEpisode = false
            self.offerNextEpisode = false
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
        creditsTask?.cancel(); creditsTask = nil
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

/// Diamond's credits analysis for one file (GET /v1/library/assets/{id}/credits).
struct CreditsDetection: Decodable {
    struct Segment: Decodable { let start: Double; let end: Double }
    let state: String
    let segments: [Segment]?
    let duration: Double?
    /// The closing credits: the last detected segment in the second half of the file.
    var closingCredits: EpisodeCredits? {
        guard state == "ready" else { return nil }
        return (segments ?? []).filter { $0.end > $0.start && $0.start > 0 }.max { $0.start < $1.start }
            .map { EpisodeCredits(start: $0.start, end: $0.end) }
    }
}

/// When to offer the next episode: at the start of the closing credits, or at
/// 90% when none were detected.
struct EpisodeCredits: Equatable {
    let start: Double
    let end: Double
    /// Credits detected in the first half are not closing credits.
    private func valid(_ duration: Double) -> Bool { start >= duration * 0.5 && start < duration }
    static func offerStart(_ credits: EpisodeCredits?, duration: Double) -> Double {
        if let credits, credits.valid(duration) { return credits.start }
        return duration * 0.9
    }
    /// Once offered, the prompt stays up until the file ends.
    static func prompting(_ credits: EpisodeCredits?, position: Double, duration: Double) -> Bool {
        position >= offerStart(credits, duration: duration)
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
        var sizeObservation: NSKeyValueObservation?
        var fullScreenObserver: Any?
        var picture = CGSize.zero
        /// The window keeps the picture's shape, so it can be sized to the movie with no black bars.
        func watch(_ player: AVPlayer) {
            sizeObservation = player.observe(\.currentItem?.presentationSize, options: [.initial, .new]) { [weak self] player, _ in
                let size = player.currentItem?.presentationSize ?? .zero
                DispatchQueue.main.async { self?.fit(size) }
            }
        }
        func fit(_ size: CGSize) {
            guard size.width > 0, size.height > 0 else { return }
            picture = size
            guard let window else { return }
            // A full-screen window is left alone; it takes the picture's shape when it leaves full screen.
            guard !window.styleMask.contains(.fullScreen) else { return }
            let shape = NSSize(width: size.width, height: size.height)
            let chrome = window.frame.height - (window.contentView?.frame.height ?? window.frame.height)
            guard window.contentAspectRatio != shape else { return }
            window.contentAspectRatio = shape
            var frame = window.frame
            let top = frame.maxY
            var width = frame.width
            var height = (width * size.height / size.width).rounded() + chrome
            if let visible = window.screen?.visibleFrame, height > visible.height {
                height = visible.height
                width = ((height - chrome) * size.width / size.height).rounded()
            }
            frame.size = NSSize(width: width, height: height)
            frame.origin.y = top - height
            window.setFrame(frame, display: true, animate: false)
        }
        func configure(_ window: NSWindow, title: String) {
            window.title = title
            guard self.window !== window else { return }
            self.window = window
            fullScreenObserver = NotificationCenter.default.addObserver(forName: NSWindow.didExitFullScreenNotification, object: window, queue: .main) { [weak self] _ in
                guard let self else { return }
                self.fit(self.picture)
            }
            fit(picture)
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
            sizeObservation = nil
            if let fullScreenObserver { NotificationCenter.default.removeObserver(fullScreenObserver) }; fullScreenObserver = nil
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
        context.coordinator.watch(player)
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
class KeyboardPlayerController: AVPlayerViewController, AVPlayerViewControllerDelegate {
    var close: (() -> Void)?
    #if os(iOS)
    /// iOS pauses a video whose player is on screen when the app leaves the foreground, so the
    /// player is taken off the view then (it keeps playing: the sound goes on with the phone
    /// locked) and put back on return. Not while Picture in Picture shows it.
    private var detached: AVPlayer?
    private var pictureInPicture = false
    override func viewDidLoad() {
        super.viewDidLoad()
        delegate = self
        NotificationCenter.default.addObserver(self, selector: #selector(leftForeground), name: UIApplication.didEnterBackgroundNotification, object: nil)
        NotificationCenter.default.addObserver(self, selector: #selector(returned), name: UIApplication.willEnterForegroundNotification, object: nil)
    }
    @objc private func leftForeground() {
        guard !pictureInPicture, let player, player.timeControlStatus != .paused else { return }
        detached = player
        self.player = nil
        // Off the view, AVKit no longer fills the lock screen: Torrbaby does.
        NowPlaying.shared.begin(player)
    }
    @objc private func returned() {
        NowPlaying.shared.end()
        if let detached, player == nil { player = detached }
        detached = nil
    }
    func playerViewControllerWillStartPictureInPicture(_ playerViewController: AVPlayerViewController) { pictureInPicture = true }
    func playerViewControllerDidStopPictureInPicture(_ playerViewController: AVPlayerViewController) { pictureInPicture = false }
    #endif
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
    var offerNext = false
    var advance: () -> Void = {}
    func makeUIViewController(context: Context) -> KeyboardPlayerController {
        let view = KeyboardPlayerController()
        view.player = player
        view.close = close
        view.showsPlaybackControls = true
        view.speeds = PlaybackSpeeds.options
        view.allowsPictureInPicturePlayback = true
        #if os(tvOS)
        // Ask the television for the video's own dynamic range and frame rate, as the
        // Apple TV's Match Content settings allow. Without this a Dolby Vision or HDR
        // video is converted to whatever format the Apple TV happens to be outputting.
        view.appliesPreferredDisplayCriteriaAutomatically = true
        #endif
        return view
    }
    func updateUIViewController(_ view: KeyboardPlayerController, context: Context) {
        // Not while the app is in the background: the player is off the view on purpose then.
        #if os(iOS)
        if UIApplication.shared.applicationState != .background { view.player = player }
        #else
        view.player = player
        #endif
        view.close = close
        #if os(tvOS)
        view.transportBarCustomMenuItems = canAdvance
            ? [UIAction(title: L10n.text("Next episode"), image: UIImage(systemName: "forward.end.fill")) { _ in advance() }] : []
        // The system's contextual action button appears over the video during
        // the credits, with or without the transport bar, and takes focus.
        let showing = !view.contextualActions.isEmpty
        if offerNext != showing {
            // Words only: the prompt reads as a plain "Next Episode" button.
            view.contextualActions = offerNext ? [UIAction(title: L10n.text("Next Episode")) { _ in advance() }] : []
        }
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
                NativePlayer(player: model.player, close: { dismiss() }, canAdvance: model.canAdvanceEpisode, offerNext: model.offerNextEpisode, advance: { model.advanceToNextEpisode() })
                #endif
            }
        }
        #if os(macOS)
        .overlay(alignment: .bottomTrailing) {
            if model.offerNextEpisode && !model.preparingNext && model.error == nil && model.endMessage == nil {
                Button { model.advanceToNextEpisode() } label: {
                    Label("Next episode", systemImage: "forward.end.fill").font(.headline).padding(.horizontal, 6).padding(.vertical, 4)
                }
                .buttonStyle(.borderedProminent)
                .tint(.white).foregroundStyle(.black)
                .padding(.trailing, 32).padding(.bottom, 96)
                .transition(.opacity)
            }
        }
        .animation(.easeInOut(duration: 0.25), value: model.offerNextEpisode)
        #endif
        .background(.black)
        #if !os(tvOS)
        .presentationBackground(.black)
        #endif
        #if os(macOS)
        // Low enough for a 2.39:1 film at the narrowest width.
        .frame(minWidth: 480, minHeight: 180)
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
    static func key(_ store: Store) -> String { "subtitle-defaults:" + store.libraryKey + ":" + store.clientId }
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
    /// Subtitles drawn over ones printed in the picture sit on a box at least this opaque.
    static let coveringOpacity = 90
    init(item: AVPlayerItem, store: Store, overPictureSubtitles: Bool = false) {
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
                    kCMTextMarkupAttribute_BackgroundColorARGB as String: [Double(overPictureSubtitles ? max(p.backgroundOpacity, Self.coveringOpacity) : p.backgroundOpacity) / 100, 0, 0, 0]
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

#if os(iOS)
/// The lock screen while a video plays with the phone locked (its player off the view, so
/// AVKit no longer publishes it): the title, the show and episode, the poster and the time,
/// from the item's own metadata, with play/pause, 15 s skips and scrubbing.
@MainActor final class NowPlaying {
    static let shared = NowPlaying()
    private weak var player: AVPlayer?
    private var targets: [(MPRemoteCommand, Any)] = []
    private var observation: NSKeyValueObservation?

    func begin(_ player: AVPlayer) {
        end()
        self.player = player
        let center = MPRemoteCommandCenter.shared()
        func on(_ command: MPRemoteCommand, _ action: @escaping (MPRemoteCommandEvent) -> MPRemoteCommandHandlerStatus) {
            command.isEnabled = true
            targets.append((command, command.addTarget(handler: action)))
        }
        on(center.playCommand) { [weak self] _ in self?.player?.play(); self?.publish(); return .success }
        on(center.pauseCommand) { [weak self] _ in self?.player?.pause(); self?.publish(); return .success }
        on(center.togglePlayPauseCommand) { [weak self] _ in
            guard let player = self?.player else { return .commandFailed }
            if player.timeControlStatus == .paused { player.play() } else { player.pause() }
            self?.publish(); return .success
        }
        center.skipForwardCommand.preferredIntervals = [15]
        center.skipBackwardCommand.preferredIntervals = [15]
        on(center.skipForwardCommand) { [weak self] _ in self?.skip(15); return .success }
        on(center.skipBackwardCommand) { [weak self] _ in self?.skip(-15); return .success }
        on(center.changePlaybackPositionCommand) { [weak self] event in
            guard let event = event as? MPChangePlaybackPositionCommandEvent else { return .commandFailed }
            self?.player?.seek(to: CMTime(seconds: event.positionTime, preferredTimescale: 600)) { _ in Task { @MainActor in self?.publish() } }
            return .success
        }
        observation = player.observe(\.timeControlStatus) { [weak self] _, _ in Task { @MainActor in self?.publish() } }
        publish()
    }

    func end() {
        for (command, target) in targets { command.removeTarget(target) }
        targets = []
        observation = nil
        player = nil
    }

    private func skip(_ seconds: Double) {
        guard let player else { return }
        let target = max(0, player.currentTime().seconds + seconds)
        player.seek(to: CMTime(seconds: target, preferredTimescale: 600)) { _ in Task { @MainActor in self.publish() } }
    }

    private func publish() {
        guard let player, let item = player.currentItem else { return }
        var info: [String: Any] = [:]
        for entry in item.externalMetadata {
            switch entry.identifier {
            case .commonIdentifierTitle?: info[MPMediaItemPropertyTitle] = entry.stringValue
            case .iTunesMetadataTrackSubTitle?: info[MPMediaItemPropertyArtist] = entry.stringValue
            case .commonIdentifierArtwork?:
                if let data = entry.dataValue, let image = UIImage(data: data) {
                    info[MPMediaItemPropertyArtwork] = MPMediaItemArtwork(boundsSize: image.size) { _ in image }
                }
            default: break
            }
        }
        let duration = item.duration.seconds
        if duration.isFinite { info[MPMediaItemPropertyPlaybackDuration] = duration }
        info[MPNowPlayingInfoPropertyElapsedPlaybackTime] = player.currentTime().seconds
        info[MPNowPlayingInfoPropertyPlaybackRate] = player.timeControlStatus == .paused ? 0.0 : Double(player.rate)
        info[MPNowPlayingInfoPropertyMediaType] = MPNowPlayingInfoMediaType.video.rawValue
        MPNowPlayingInfoCenter.default().nowPlayingInfo = info
    }
}
#endif
