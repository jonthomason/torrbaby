#if os(iOS)
import SwiftUI
import AVKit
import Combine

/// Presents AVKit itself, rather than embedding it in a SwiftUI sheet.
/// Retains the playback session while AVKit is running in Picture in Picture.
struct MobilePlayerPresentation: UIViewControllerRepresentable {
    @Binding var playback: Playback?
    let store: Store

    final class Host: UIViewController {
        var appeared: (() -> Void)?
        override func viewDidAppear(_ animated: Bool) { super.viewDidAppear(animated); appeared?() }
    }
    final class PlayerController: KeyboardPlayerController {
        var disappeared: (() -> Void)?
        override func viewDidDisappear(_ animated: Bool) {
            super.viewDidDisappear(animated)
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
            host.present(controller, animated: true)
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
            if let backgroundObserver { NotificationCenter.default.removeObserver(backgroundObserver); self.backgroundObserver = nil }
            if Self.active === self { Self.active = nil }
        }
        func playerViewControllerWillStartPictureInPicture(_ playerViewController: AVPlayerViewController) {
            pictureInPicture = true
            model?.report()
        }
        func playerViewControllerShouldAutomaticallyDismissAtPictureInPictureStart(_ playerViewController: AVPlayerViewController) -> Bool { true }
        func playerViewController(_ playerViewController: AVPlayerViewController, failedToStartPictureInPictureWithError error: Error) {
            pictureInPicture = false
            parent.store.actionError = error.localizedDescription
        }
        func playerViewControllerDidStopPictureInPicture(_ playerViewController: AVPlayerViewController) {
            pictureInPicture = false
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
