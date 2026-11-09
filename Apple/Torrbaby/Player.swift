import SwiftUI
import AVKit

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
    private var observation: NSKeyValueObservation?
    private var timeObserver: Any?
    private var rateObservation: NSKeyValueObservation?
    private var endObserver: NSObjectProtocol?
    private var sequence = 0
    private var starting = false
    private var resumed = false
    private var finished = false
    private var playback: Playback
    private let store: Store
    init(playback: Playback, store: Store) {
        self.playback = playback
        self.store = store
        self.title = playback.name
        player = AVPlayer()
        load(playback)
        #if os(iOS) || os(tvOS)
        try? AVAudioSession.sharedInstance().setCategory(.playback, mode: .moviePlayback)
        try? AVAudioSession.sharedInstance().setActive(true)
        #endif
        timeObserver = player.addPeriodicTimeObserver(forInterval: CMTime(seconds: 5, preferredTimescale: 600), queue: .main) { [weak self] _ in
            Task { @MainActor [weak self] in self?.report() }
        }
        rateObservation = player.observe(\.rate, options: [.new]) { [weak self] _, _ in
            Task { @MainActor [weak self] in self?.report() }
        }
    }
    private func load(_ next: Playback) {
        if let endObserver { NotificationCenter.default.removeObserver(endObserver) }
        observation = nil
        playback = next; title = next.name; error = nil; endMessage = nil
        sequence = 0; starting = false; resumed = false
        let url = playback.url
        let title = playback.name
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
            Task { @MainActor [weak self] in
                guard let self, !self.finished, self.playback.id == playbackID else { return }
                self.report(ended: true, reachedEnd: true)
                #if !os(tvOS)
                let queue = DownloadQueue.shared
                let episode = queue.isEpisode(self.playback)
                if self.playback.localDownloadID != nil || episode {
                    self.resumed = false
                    self.player.replaceCurrentItem(with: nil)
                    if let id = self.playback.localDownloadID { queue.finished(id) }
                    if episode, let next = await queue.nextPlayback(after: self.playback) {
                        guard !self.finished else { return }
                        self.load(next)
                    } else if !self.finished {
                        self.endMessage = episode ? "Next episode unavailable" : "Playback finished"
                    }
                }
                #endif
            }
        }
        observation = item.observe(\.status, options: [.new, .initial]) { [weak self] item, _ in
            if item.status == .readyToPlay {
                Task { @MainActor [weak self] in
                    guard let self, self.player.currentItem === item else { return }
                    self.start()
                }
            }
            if item.status == .failed {
                let message = item.error?.localizedDescription ?? "Apple's player cannot open this file."
                Task { @MainActor [weak self] in self?.error = message }
            }
        }
    }
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
    func report(ended: Bool = false, reachedEnd: Bool = false) {
        guard resumed, !finished else { return }
        let duration = player.currentItem?.duration.seconds ?? 0
        let position = player.currentTime().seconds
        guard duration.isFinite, duration > 0, position.isFinite else { return }
        #if !os(tvOS)
        if let id = playback.localDownloadID {
            DownloadQueue.shared.report(id, position: position, duration: duration, reachedEnd: reachedEnd)
            return
        }
        #endif
        guard let session = playback.sessionId else { return }
        sequence += 1
        store.reportProgress(sessionId: session, sequence: sequence, position: position, duration: duration, ended: ended)
    }
    func stop() {
        player.pause()
        report(ended: true)
        finished = true
        if let timeObserver { player.removeTimeObserver(timeObserver); self.timeObserver = nil }
        rateObservation = nil
        observation = nil
        if let endObserver { NotificationCenter.default.removeObserver(endObserver); self.endObserver = nil }
    }
}

#if os(macOS)
struct NativePlayer: NSViewRepresentable {
    let title: String
    let player: AVPlayer
    let close: () -> Void
    class Coordinator {
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
        if let window = view.window { context.coordinator.configure(window, title: title) }
    }
    static func dismantleNSView(_ view: AVPlayerView, coordinator: Coordinator) { coordinator.stop(); view.player = nil }
}
#else
final class KeyboardPlayerController: AVPlayerViewController {
    var close: (() -> Void)?
    override var canBecomeFirstResponder: Bool { true }
    override func viewDidAppear(_ animated: Bool) { super.viewDidAppear(animated); becomeFirstResponder() }
    override var keyCommands: [UIKeyCommand]? {
        let commands = [
            UIKeyCommand(input: UIKeyCommand.inputLeftArrow, modifierFlags: [], action: #selector(back)),
            UIKeyCommand(input: UIKeyCommand.inputRightArrow, modifierFlags: [], action: #selector(forward)),
            UIKeyCommand(input: UIKeyCommand.inputEscape, modifierFlags: [], action: #selector(exitPlayer))
        ]
        commands.forEach { $0.wantsPriorityOverSystemBehavior = true }
        return commands
    }
    @objc private func back() { if let player { skip(player, seconds: -15) } }
    @objc private func forward() { if let player { skip(player, seconds: 15) } }
    @objc private func exitPlayer() { close?() }
}
struct NativePlayer: UIViewControllerRepresentable {
    let player: AVPlayer
    let close: () -> Void
    func makeUIViewController(context: Context) -> KeyboardPlayerController {
        let view = KeyboardPlayerController()
        view.player = player
        view.close = close
        view.showsPlaybackControls = true
        view.allowsPictureInPicturePlayback = true
        return view
    }
    func updateUIViewController(_ view: KeyboardPlayerController, context: Context) { view.player = player; view.close = close }
}
#endif

struct PlayerScreen: View {
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
            if let error = model.error {
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
                NativePlayer(title: model.title, player: model.player, close: { dismiss() })
                #else
                NativePlayer(player: model.player, close: { dismiss() })
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
        .onDisappear { model.stop() }
    }
}
