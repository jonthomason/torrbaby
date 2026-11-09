import UIKit
import AVKit
import AVFoundation

@main final class App: UIResponder, UIApplicationDelegate {
    var window: UIWindow?
    func application(_ application: UIApplication, didFinishLaunchingWithOptions options: [UIApplication.LaunchOptionsKey: Any]? = nil) -> Bool {
        let window = UIWindow(frame: UIScreen.main.bounds)
        window.rootViewController = ProbeController(); window.makeKeyAndVisible(); self.window = window
        return true
    }
}
final class ProbeController: UIViewController, AVPlayerViewControllerDelegate {
    let video = AVPlayerViewController()
    let status = UILabel()
    var report = ""
    let captions = UILabel()
    let languages = UISegmentedControl(items: ["Off", "English", "中文"])
    var clock: Any?
    var choice = 1
    @objc func selectLanguage() { choice = languages.selectedSegmentIndex; updateCaption() }
    func updateCaption() {
        let second = video.player?.currentTime().seconds ?? 0
        captions.text = choice == 0 ? nil : choice == 1 ? "English • external subtitle • \(Int(second))s" : "中文 • 外部字幕 • \(Int(second))秒"
    }
    func playerViewControllerDidStartPictureInPicture(_ playerViewController: AVPlayerViewController) { note("PiP started") }
    func playerViewControllerDidStopPictureInPicture(_ playerViewController: AVPlayerViewController) { note("PiP stopped") }
    func playerViewController(_ playerViewController: AVPlayerViewController, failedToStartPictureInPictureWithError error: Error) { note("PiP error: \(error.localizedDescription)") }
    func playerViewController(_ playerViewController: AVPlayerViewController, restoreUserInterfaceForPictureInPictureStopWithCompletionHandler completionHandler: @escaping (Bool) -> Void) { completionHandler(true) }
    func note(_ text: String) {
        report += text + "\n"; status.text = report
        try? report.write(to: FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0].appendingPathComponent("report.txt"), atomically: true, encoding: .utf8)
    }
    override func viewDidLoad() {
        super.viewDidLoad(); view.backgroundColor = .black
        addChild(video); view.addSubview(video.view); video.didMove(toParent: self)
        video.view.translatesAutoresizingMaskIntoConstraints = false
        status.textColor = .white; status.font = .monospacedSystemFont(ofSize: 16, weight: .regular); status.numberOfLines = 0
        status.translatesAutoresizingMaskIntoConstraints = false; view.addSubview(status)
        languages.selectedSegmentIndex = 1
        languages.addTarget(self, action: #selector(selectLanguage), for: .valueChanged)
        languages.translatesAutoresizingMaskIntoConstraints = false; view.addSubview(languages)
        video.delegate = self
        captions.font = .systemFont(ofSize: 26, weight: .medium)
        captions.textColor = .white; captions.backgroundColor = UIColor.black.withAlphaComponent(0.65)
        captions.textAlignment = .center; captions.translatesAutoresizingMaskIntoConstraints = false
        if let overlay = video.contentOverlayView {
            overlay.addSubview(captions)
            NSLayoutConstraint.activate([captions.centerXAnchor.constraint(equalTo: overlay.centerXAnchor), captions.bottomAnchor.constraint(equalTo: overlay.bottomAnchor, constant: -90)])
        }
        NSLayoutConstraint.activate([
            status.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor, constant: 12),
            status.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 20), status.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -20),
            languages.topAnchor.constraint(equalTo: status.bottomAnchor, constant: 12), languages.centerXAnchor.constraint(equalTo: view.centerXAnchor), video.view.topAnchor.constraint(equalTo: languages.bottomAnchor, constant: 12), video.view.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            video.view.trailingAnchor.constraint(equalTo: view.trailingAnchor), video.view.bottomAnchor.constraint(equalTo: view.bottomAnchor)])
        Task { do { try await start() } catch { note("Error: \(error.localizedDescription)") } }
    }
    func start() async throws {
        let began = Date()
        try AVAudioSession.sharedInstance().setCategory(.playback, mode: .moviePlayback)
        try AVAudioSession.sharedInstance().setActive(true)
        let movie = AVURLAsset(url: Bundle.main.url(forResource: "sample", withExtension: "mp4")!)
        let subtitles = AVURLAsset(url: Bundle.main.url(forResource: "captions", withExtension: "mp4")!)
        let composition = AVMutableComposition()
        for source in try await movie.load(.tracks) where source.mediaType == .video || source.mediaType == .audio {
            let track = composition.addMutableTrack(withMediaType: source.mediaType, preferredTrackID: kCMPersistentTrackID_Invalid)!
            try track.insertTimeRange(try await source.load(.timeRange), of: source, at: .zero)
            track.preferredTransform = try await source.load(.preferredTransform)
        }
        for source in try await subtitles.load(.tracks) {
            let track = composition.addMutableTrack(withMediaType: source.mediaType, preferredTrackID: kCMPersistentTrackID_Invalid)!
            try track.insertTimeRange(try await source.load(.timeRange), of: source, at: .zero)
            track.languageCode = try await source.load(.languageCode)
        }
        note("External subtitle composition — isolated experiment")
        let group = try await composition.loadMediaSelectionGroup(for: .legible)
        note("Native subtitle choices: \(group?.options.count ?? 0)")
        note("PiP supported by environment: \(AVPictureInPictureController.isPictureInPictureSupported())")
        let item = AVPlayerItem(asset: composition)
        video.player = AVPlayer(playerItem: item)
        video.player?.isMuted = true
        clock = video.player?.addPeriodicTimeObserver(forInterval: CMTime(seconds: 0.25, preferredTimescale: 600), queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.updateCaption() }
        }
        video.allowsPictureInPicturePlayback = true
        video.canStartPictureInPictureAutomaticallyFromInline = true
        for _ in 0..<100 {
            if item.status != .unknown { break }
            try await Task.sleep(for: .milliseconds(100))
        }
        note("Ready: \(item.status == .readyToPlay), \(String(format: "%.2f", Date().timeIntervalSince(began)))s")
        if let error = item.error { note(error.localizedDescription) }
        // Pause in the first subtitle cue for visual inspection.
        let sought = await video.player!.seek(to: CMTime(seconds: 2, preferredTimescale: 600), toleranceBefore: .zero, toleranceAfter: .zero)
        note("Seek to first cue: \(sought)")
        video.player?.play()
    }
}
