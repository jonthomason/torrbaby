#if os(iOS)
import SwiftUI
import AVKit
import Combine

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
    final class PlayerController: KeyboardPlayerController, UIGestureRecognizerDelegate {
        var disappeared: (() -> Void)?
        var advanceEpisode: (() -> Void)?
        var nextEpisodeAvailable = false { didSet { updateNextButton() } }
        var inPictureInPicture = false { didSet { updateNextButton() } }
        private let nextButton = UIButton(type: .system)
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
                || Date() >= interactionDeadline
        }
        func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer,
                               shouldRecognizeSimultaneouslyWith otherGestureRecognizer: UIGestureRecognizer) -> Bool { true }
        func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer, shouldReceive touch: UITouch) -> Bool {
            // Native control touches (including scrubbing) should reveal the action too.
            showNextButton()
            return !(touch.view?.isDescendant(of: nextButton) ?? false)
        }
        override func viewDidDisappear(_ animated: Bool) {
            super.viewDidDisappear(animated)
            hideNextButton?.cancel()
            interactionDeadline = .distantPast
            nextButton.isHidden = true
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
            host.present(controller, animated: true)
        }
        private func fitPictureToScreen() {
            guard let controller, let model, UIDevice.current.userInterfaceIdiom == .phone else { return }
            let screen = UIScreen.main.bounds.size
            let aspect = Double(max(screen.width, screen.height) / max(1, min(screen.width, screen.height)))
            controller.videoGravity = model.video?.fills(screenAspect: aspect) == true ? .resizeAspectFill : .resizeAspect
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
            nextEpisodeObservation = nil
            preparationSpinner?.removeFromSuperview(); preparationSpinner = nil
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
