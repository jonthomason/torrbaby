#if os(iOS)
import SwiftUI
import AVKit
import Combine

/// iPhone shows its library and settings in portrait only; the player alone rotates.
/// iPad is unaffected.
@MainActor enum PhoneOrientation {
    static var playerVisible = false
    static var allowed: UIInterfaceOrientationMask {
        guard UIDevice.current.userInterfaceIdiom == .phone else { return .all }
        return playerVisible ? .allButUpsideDown : .portrait
    }
    static func returnToPortrait() {
        guard UIDevice.current.userInterfaceIdiom == .phone else { return }
        for scene in UIApplication.shared.connectedScenes.compactMap({ $0 as? UIWindowScene }) {
            scene.requestGeometryUpdate(.iOS(interfaceOrientations: .portrait)) { _ in }
            for window in scene.windows { window.rootViewController?.setNeedsUpdateOfSupportedInterfaceOrientations() }
        }
    }
}
/// Presents AVKit itself, rather than embedding it in a SwiftUI sheet.
/// Retains the playback session while AVKit is running in Picture in Picture.
struct MobilePlayerPresentation: UIViewControllerRepresentable {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @Binding var playback: Playback?
    let store: Store

    final class Host: UIViewController {
        var appeared: (() -> Void)?
        override func viewDidAppear(_ animated: Bool) { super.viewDidAppear(animated); appeared?() }
    }
    /// Screen brightness, as in Netflix's player: swipe up or down on the left of the picture.
    /// AVKit's own controls cannot take a slider, so this is a gesture with a small level
    /// that shows while it changes. The brightness from before is put back when the player
    /// closes. Swipes elsewhere (and the swipe down that closes the player) are untouched.
    @MainActor final class BrightnessControl: NSObject, UIGestureRecognizerDelegate {
        private weak var view: UIView?
        private let level = UIView()
        private let fill = UIView()
        private var fillHeight: NSLayoutConstraint?
        private var startValue: CGFloat = 0
        private var original: CGFloat?
        private var hide: Task<Void, Never>?
        private let height: CGFloat = 150
        private var screen: UIScreen? { view?.window?.windowScene?.screen }

        func install(in view: UIView) {
            self.view = view
            let pan = UIPanGestureRecognizer(target: self, action: #selector(pan(_:)))
            pan.delegate = self
            pan.maximumNumberOfTouches = 1
            view.addGestureRecognizer(pan)
            let blur = UIVisualEffectView(effect: UIBlurEffect(style: .systemUltraThinMaterialDark))
            blur.translatesAutoresizingMaskIntoConstraints = false
            blur.layer.cornerRadius = 18; blur.clipsToBounds = true
            let track = UIView(); track.translatesAutoresizingMaskIntoConstraints = false
            track.backgroundColor = UIColor.white.withAlphaComponent(0.25); track.layer.cornerRadius = 3; track.clipsToBounds = true
            fill.translatesAutoresizingMaskIntoConstraints = false; fill.backgroundColor = .white
            let icon = UIImageView(image: UIImage(systemName: "sun.max.fill")); icon.translatesAutoresizingMaskIntoConstraints = false
            icon.tintColor = .white; icon.contentMode = .scaleAspectFit
            level.translatesAutoresizingMaskIntoConstraints = false
            level.isUserInteractionEnabled = false; level.alpha = 0
            level.isAccessibilityElement = false
            view.addSubview(level); level.addSubview(blur); blur.contentView.addSubview(track); track.addSubview(fill); blur.contentView.addSubview(icon)
            let fillHeight = fill.heightAnchor.constraint(equalToConstant: 0); self.fillHeight = fillHeight
            NSLayoutConstraint.activate([
                level.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 28),
                level.centerYAnchor.constraint(equalTo: view.centerYAnchor),
                level.widthAnchor.constraint(equalToConstant: 36),
                level.heightAnchor.constraint(equalToConstant: height + 40),
                blur.leadingAnchor.constraint(equalTo: level.leadingAnchor), blur.trailingAnchor.constraint(equalTo: level.trailingAnchor),
                blur.topAnchor.constraint(equalTo: level.topAnchor), blur.bottomAnchor.constraint(equalTo: level.bottomAnchor),
                track.centerXAnchor.constraint(equalTo: blur.contentView.centerXAnchor),
                track.topAnchor.constraint(equalTo: blur.contentView.topAnchor, constant: 10),
                track.widthAnchor.constraint(equalToConstant: 6), track.heightAnchor.constraint(equalToConstant: height),
                fill.leadingAnchor.constraint(equalTo: track.leadingAnchor), fill.trailingAnchor.constraint(equalTo: track.trailingAnchor),
                fill.bottomAnchor.constraint(equalTo: track.bottomAnchor), fillHeight,
                icon.centerXAnchor.constraint(equalTo: blur.contentView.centerXAnchor),
                icon.bottomAnchor.constraint(equalTo: blur.contentView.bottomAnchor, constant: -8),
                icon.widthAnchor.constraint(equalToConstant: 16), icon.heightAnchor.constraint(equalToConstant: 16),
            ])
        }
        func bringToFront() { if let view { view.bringSubviewToFront(level) } }
        func restore() {
            if let original, let screen { screen.brightness = original }
            original = nil
        }
        @objc private func pan(_ gesture: UIPanGestureRecognizer) {
            guard let view, let screen else { return }
            switch gesture.state {
            case .began:
                if original == nil { original = screen.brightness }
                startValue = screen.brightness
                show(screen.brightness)
            case .changed:
                // Up brightens; the full range takes about 60% of the screen's height.
                let change = -gesture.translation(in: view).y / max(200, view.bounds.height * 0.6)
                let value = min(1, max(0, startValue + change))
                screen.brightness = value
                show(value)
            default:
                hide?.cancel()
                hide = Task { @MainActor [weak self] in
                    do { try await Task.sleep(for: .seconds(1)) } catch { return }
                    UIView.animate(withDuration: 0.3) { self?.level.alpha = 0 }
                }
            }
        }
        private func show(_ value: CGFloat) {
            hide?.cancel()
            fillHeight?.constant = height * value
            if level.alpha < 1 { UIView.animate(withDuration: 0.15) { self.level.alpha = 1 } }
        }
        func gestureRecognizerShouldBegin(_ gestureRecognizer: UIGestureRecognizer) -> Bool {
            guard let view, let pan = gestureRecognizer as? UIPanGestureRecognizer else { return false }
            let point = pan.location(in: view), velocity = pan.velocity(in: view)
            // The left third, away from the top and bottom edges (system gestures, the timeline).
            return point.x < view.bounds.width / 3 && point.y > 70 && point.y < view.bounds.height - 110
                && abs(velocity.y) > abs(velocity.x) * 1.3
        }
        /// While it is changing brightness, AVKit's own swipes (closing the player) wait.
        func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer, shouldBeRequiredToFailBy other: UIGestureRecognizer) -> Bool {
            other is UIPanGestureRecognizer
        }
    }
    final class PlayerController: KeyboardPlayerController, UIGestureRecognizerDelegate {
        var disappeared: (() -> Void)?
        /// Called when the player's size changes (rotation, Split View).
        var sizeChanged: (() -> Void)?
        private var lastSize = CGSize.zero
        var advanceEpisode: (() -> Void)?
        var nextEpisodeAvailable = false { didSet { updateNextButton() } }
        var inPictureInPicture = false { didSet { pictureInPicture = inPictureInPicture; updateNextButton() } }
        /// During the closing credits the button shows without a tap.
        var creditsPrompt = false { didSet { updateNextButton() } }
        private let nextButton = UIButton(type: .system)
        private let brightness = BrightnessControl()
        private var interactionDeadline = Date.distantPast
        private var hideNextButton: Task<Void, Never>?

        override func viewDidLoad() {
            super.viewDidLoad()
            var configuration = UIButton.Configuration.filled()
            configuration.title = L10n.text("Next Episode")
            configuration.image = UIImage(systemName: "forward.end.fill")
            configuration.imagePadding = 10
            configuration.baseBackgroundColor = .white
            configuration.baseForegroundColor = .black
            configuration.cornerStyle = .capsule
            configuration.contentInsets = NSDirectionalEdgeInsets(top: 14, leading: 20, bottom: 14, trailing: 20)
            nextButton.configuration = configuration
            nextButton.translatesAutoresizingMaskIntoConstraints = false
            nextButton.isHidden = true
            nextButton.addAction(UIAction { [weak self] _ in
                guard let self, self.nextEpisodeAvailable, !self.inPictureInPicture else { return }
                self.nextEpisodeAvailable = false
                self.advanceEpisode?()
            }, for: .touchUpInside)
            // A sibling above AVKit's view content receives touches without changing
            // its native controls. contentOverlayView is for noninteractive content.
            view.addSubview(nextButton)
            NSLayoutConstraint.activate([
                nextButton.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -24),
                nextButton.bottomAnchor.constraint(equalTo: view.safeAreaLayoutGuide.bottomAnchor, constant: -130)
            ])
            brightness.install(in: view)
            let interaction = UITapGestureRecognizer(target: self, action: #selector(showNextButton))
            interaction.cancelsTouchesInView = false
            interaction.delegate = self
            view.addGestureRecognizer(interaction)
        }
        override func viewDidAppear(_ animated: Bool) {
            super.viewDidAppear(animated)
            showNextButton()
        }
        override func viewDidLayoutSubviews() {
            super.viewDidLayoutSubviews()
            view.bringSubviewToFront(nextButton)
            brightness.bringToFront()
            if view.bounds.size != lastSize { lastSize = view.bounds.size; sizeChanged?() }
        }
        @objc private func showNextButton() {
            interactionDeadline = Date().addingTimeInterval(6)
            updateNextButton()
            hideNextButton?.cancel()
            hideNextButton = Task { @MainActor [weak self] in
                do { try await Task.sleep(for: .seconds(6)) } catch { return }
                self?.updateNextButton()
            }
        }
        private func updateNextButton() {
            nextButton.isHidden = !nextEpisodeAvailable || inPictureInPicture
                || (Date() >= interactionDeadline && !creditsPrompt)
        }
        func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer,
                               shouldRecognizeSimultaneouslyWith otherGestureRecognizer: UIGestureRecognizer) -> Bool { true }
        func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer, shouldReceive touch: UITouch) -> Bool {
            // Native control touches (including scrubbing) should reveal the action too.
            showNextButton()
            return !(touch.view?.isDescendant(of: nextButton) ?? false)
        }
        // iPhone: the player is the only screen that turns; everything else is portrait.
        override func viewWillAppear(_ animated: Bool) {
            super.viewWillAppear(animated)
            PhoneOrientation.playerVisible = true
            setNeedsUpdateOfSupportedInterfaceOrientations()
        }
        override func viewWillDisappear(_ animated: Bool) {
            super.viewWillDisappear(animated)
            guard isBeingDismissed || presentingViewController == nil else { return }
            PhoneOrientation.playerVisible = false
            PhoneOrientation.returnToPortrait()
        }
        override func viewDidDisappear(_ animated: Bool) {
            super.viewDidDisappear(animated)
            PhoneOrientation.playerVisible = false
            PhoneOrientation.returnToPortrait()
            hideNextButton?.cancel()
            interactionDeadline = .distantPast
            nextButton.isHidden = true
            // The screen goes back to the brightness it had before the player changed it.
            if !inPictureInPicture { brightness.restore() }
            disappeared?()
        }

    }
    @MainActor
    final class Coordinator: NSObject, AVPlayerViewControllerDelegate {
        // AVKit's delegate is weak. Keep the session alive if the originating page
        // disappears while PiP continues above the library or another app.
        static var active: Coordinator?
        var parent: MobilePlayerPresentation
        weak var host: Host?
        weak var window: UIWindow?
        var controller: PlayerController?
        var model: PlaybackModel?
        private var creditsObservation: AnyCancellable?
        var playbackID: UUID?
        var pictureInPicture = false
        var restoring = false
        var errorObservation: AnyCancellable?
        var preparationObservation: AnyCancellable?
        var completionObservation: AnyCancellable?
        var closeObservation: AnyCancellable?
        var fillObservation: AnyCancellable?
        var nextEpisodeObservation: AnyCancellable?
        weak var preparationSpinner: UIActivityIndicatorView?
        /// While the player waits for the file. Below AVKit's centre buttons, which stay where
        /// they are (play/pause and skips), so the two never sit on each other.
        weak var waitingIndicator: UIView?
        var waitingObservation: AnyCancellable?
        var backgroundObserver: NSObjectProtocol?
        init(_ parent: MobilePlayerPresentation) { self.parent = parent }
        func presentIfNeeded() {
            guard let playback = parent.playback, playbackID != playback.id,
                  let host, host.viewIfLoaded?.window != nil, host.presentedViewController == nil else { return }
            if let previous = Self.active, previous.model != nil {
                previous.close { [weak self] in self?.presentIfNeeded() }
                return
            }
            window = host.view.window
            playbackID = playback.id
            let model = PlaybackModel(playback: playback, store: parent.store)
            self.model = model
            let controller = PlayerController()
            controller.player = model.player
            controller.delegate = self
            controller.modalPresentationStyle = .fullScreen
            controller.showsPlaybackControls = true
            controller.allowsPictureInPicturePlayback = true
            controller.canStartPictureInPictureAutomaticallyFromInline = true
            controller.close = { [weak self] in self?.close() }
            controller.disappeared = { [weak self, weak controller] in
                guard let self, let controller, !self.pictureInPicture, !self.restoring,
                      controller.isBeingDismissed || controller.presentingViewController == nil else { return }
                self.finish()
            }
            self.controller = controller
            controller.advanceEpisode = { [weak model] in model?.advanceToNextEpisode() }
            nextEpisodeObservation = model.$canAdvanceEpisode.removeDuplicates().sink { [weak controller] available in
                controller?.nextEpisodeAvailable = available
            }
            creditsObservation = model.$offerNextEpisode.removeDuplicates().sink { [weak controller] prompt in
                controller?.creditsPrompt = prompt
            }
            Self.active = self
            backgroundObserver = NotificationCenter.default.addObserver(forName: UIApplication.willResignActiveNotification,
                                                                         object: nil, queue: .main) { [weak self] _ in
                Task { @MainActor in self?.model?.report() }
            }
            errorObservation = model.$error.compactMap { $0 }.sink { [weak self] message in
                guard let self else { return }
                self.parent.store.actionError = message
                self.close()
            }
            controller.loadViewIfNeeded()
            if let overlay = controller.contentOverlayView {
                let spinner = UIActivityIndicatorView(style: .large)
                spinner.color = .white
                spinner.hidesWhenStopped = true
                spinner.accessibilityLabel = L10n.text("Preparing next episode")
                spinner.translatesAutoresizingMaskIntoConstraints = false
                overlay.addSubview(spinner)
                NSLayoutConstraint.activate([
                    spinner.centerXAnchor.constraint(equalTo: overlay.centerXAnchor),
                    spinner.centerYAnchor.constraint(equalTo: overlay.centerYAnchor)
                ])
                preparationSpinner = spinner
                preparationObservation = model.$preparingNext.sink { [weak spinner] preparing in
                    if preparing { spinner?.startAnimating() } else { spinner?.stopAnimating() }
                }
                let backing = UIView()
                backing.backgroundColor = UIColor.black.withAlphaComponent(0.55)
                backing.layer.cornerRadius = 14
                backing.translatesAutoresizingMaskIntoConstraints = false
                backing.alpha = 0
                backing.isAccessibilityElement = true
                backing.accessibilityLabel = L10n.text("Loading…")
                let waitingSpinner = UIActivityIndicatorView(style: .large)
                waitingSpinner.color = .white
                waitingSpinner.startAnimating()
                waitingSpinner.translatesAutoresizingMaskIntoConstraints = false
                backing.addSubview(waitingSpinner)
                overlay.addSubview(backing)
                NSLayoutConstraint.activate([
                    backing.widthAnchor.constraint(equalToConstant: 72), backing.heightAnchor.constraint(equalToConstant: 72),
                    backing.centerXAnchor.constraint(equalTo: overlay.centerXAnchor),
                    backing.centerYAnchor.constraint(equalTo: overlay.centerYAnchor, constant: 100),
                    waitingSpinner.centerXAnchor.constraint(equalTo: backing.centerXAnchor),
                    waitingSpinner.centerYAnchor.constraint(equalTo: backing.centerYAnchor),
                ])
                waitingIndicator = backing
                waitingObservation = model.$waiting.removeDuplicates().sink { [weak backing] waiting in
                    UIView.animate(withDuration: 0.2) { backing?.alpha = waiting ? 1 : 0 }
                }
            }
            completionObservation = model.$endMessage.compactMap { $0 }.sink { [weak self] message in
                guard let self, let controller = self.controller else { return }
                let alert = UIAlertController(title: message, message: nil, preferredStyle: .alert)
                alert.addAction(UIAlertAction(title: L10n.text("Close"), style: .default) { [weak self] _ in self?.close() })
                controller.present(alert, animated: true)
            }
            closeObservation = model.$closeRequested.filter { $0 }.sink { [weak self] _ in self?.close() }
            // On a phone, a show with black bars baked into its frame would sit in a window with
            // bars on all four sides; zoom it to fill when that trims only those bars. Each new
            // episode is measured again, and a double tap still switches back.
            fillObservation = model.$title.sink { [weak self] _ in DispatchQueue.main.async { self?.fitPictureToScreen() } }
            controller.sizeChanged = { [weak self] in self?.fitPictureToScreen() }
            host.present(controller, animated: true)
        }
        private func fitPictureToScreen() {
            guard let controller, let model, UIDevice.current.userInterfaceIdiom == .phone else { return }
            // The player's own shape right now: held upright, filling would cut most of the picture away.
            let size = controller.view.bounds.size
            let fill = size.height > 0 && model.video?.fills(screenAspect: Double(size.width / size.height)) == true
            let gravity: AVLayerVideoGravity = fill ? .resizeAspectFill : .resizeAspect
            if controller.videoGravity != gravity { controller.videoGravity = gravity }
        }
        func close(completion: (() -> Void)? = nil) {
            let view = controller
            finish()
            if view?.presentingViewController != nil { view?.dismiss(animated: true, completion: completion) }
            else { completion?() }
        }
        private func finish() {
            guard model != nil else { return }
            model?.stop()
            controller?.player = nil
            controller?.delegate = nil
            if parent.playback?.id == playbackID { parent.playback = nil }
            model = nil; controller = nil; playbackID = nil; pictureInPicture = false
            errorObservation = nil
            preparationObservation = nil; completionObservation = nil; closeObservation = nil; fillObservation = nil
            nextEpisodeObservation = nil; creditsObservation = nil
            preparationSpinner?.removeFromSuperview(); preparationSpinner = nil
            waitingObservation = nil
            waitingIndicator?.removeFromSuperview(); waitingIndicator = nil
            if let backgroundObserver { NotificationCenter.default.removeObserver(backgroundObserver); self.backgroundObserver = nil }
            if Self.active === self { Self.active = nil }
        }
        func playerViewControllerWillStartPictureInPicture(_ playerViewController: AVPlayerViewController) {
            pictureInPicture = true
            controller?.inPictureInPicture = true
            model?.report()
        }
        func playerViewControllerShouldAutomaticallyDismissAtPictureInPictureStart(_ playerViewController: AVPlayerViewController) -> Bool { true }
        func playerViewController(_ playerViewController: AVPlayerViewController, failedToStartPictureInPictureWithError error: Error) {
            pictureInPicture = false
            controller?.inPictureInPicture = false
            parent.store.actionError = error.localizedDescription
        }
        func playerViewControllerDidStopPictureInPicture(_ playerViewController: AVPlayerViewController) {
            pictureInPicture = false
            controller?.inPictureInPicture = false
            model?.report()
            if !restoring && playerViewController.presentingViewController == nil { finish() }
        }
        func playerViewController(_ playerViewController: AVPlayerViewController,
                                  restoreUserInterfaceForPictureInPictureStopWithCompletionHandler completionHandler: @escaping (Bool) -> Void) {
            guard model != nil else { completionHandler(false); return }
            if playerViewController.presentingViewController != nil { completionHandler(true); return }
            var presenter: UIViewController? = host?.viewIfLoaded?.window != nil ? host : window?.rootViewController
            while let presented = presenter?.presentedViewController { presenter = presented }
            guard let presenter, presenter.viewIfLoaded?.window != nil else { completionHandler(false); return }
            restoring = true
            presenter.present(playerViewController, animated: true) { [weak self] in
                self?.restoring = false
                completionHandler(true)
            }
        }
    }
    func makeCoordinator() -> Coordinator { Coordinator(self) }
    func makeUIViewController(context: Context) -> Host {
        let host = Host()
        context.coordinator.host = host
        host.appeared = { [weak coordinator = context.coordinator] in coordinator?.presentIfNeeded() }
        return host
    }
    func updateUIViewController(_ host: Host, context: Context) {
        context.coordinator.parent = self
        DispatchQueue.main.async { [weak coordinator = context.coordinator] in coordinator?.presentIfNeeded() }
    }
}
#endif
