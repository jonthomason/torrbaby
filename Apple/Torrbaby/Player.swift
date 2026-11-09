import SwiftUI
import AVKit

@MainActor
final class PlaybackModel: ObservableObject {
    let player: AVPlayer
    @Published var error: String?
    private var observation: NSKeyValueObservation?
    init(url: URL) {
        let item = AVPlayerItem(url: url)
        player = AVPlayer(playerItem: item)
        observation = item.observe(\.status, options: [.new, .initial]) { [weak self] item, _ in
            if item.status == .failed {
                let message = item.error?.localizedDescription ?? "Apple's player cannot open this file."
                Task { @MainActor [weak self] in self?.error = message }
            }
        }
        #if os(iOS)
        try? AVAudioSession.sharedInstance().setCategory(.playback, mode: .moviePlayback)
        try? AVAudioSession.sharedInstance().setActive(true)
        #endif
        player.play()
    }
}

#if os(macOS)
struct NativePlayer: NSViewRepresentable {
    let player: AVPlayer
    func makeNSView(context: Context) -> AVPlayerView {
        let view = AVPlayerView()
        view.player = player
        view.controlsStyle = .default
        view.showsFullScreenToggleButton = true
        view.allowsPictureInPicturePlayback = true
        return view
    }
    func updateNSView(_ view: AVPlayerView, context: Context) { view.player = player }
}
#else
struct NativePlayer: UIViewControllerRepresentable {
    let player: AVPlayer
    func makeUIViewController(context: Context) -> AVPlayerViewController {
        let view = AVPlayerViewController()
        view.player = player
        view.showsPlaybackControls = true
        view.allowsPictureInPicturePlayback = true
        return view
    }
    func updateUIViewController(_ view: AVPlayerViewController, context: Context) { view.player = player }
}
#endif

struct PlayerScreen: View {
    let playback: Playback
    @StateObject private var model: PlaybackModel
    @Environment(\.dismiss) private var dismiss
    init(playback: Playback) {
        self.playback = playback
        _model = StateObject(wrappedValue: PlaybackModel(url: playback.url))
    }
    var body: some View {
        NavigationStack {
            Group {
                if let error = model.error {
                    ContentUnavailableView {
                        Label("This file couldn’t play", systemImage: "play.slash")
                    } description: {
                        Text(error + "\n\nThis prototype plays the original file with Apple’s player. It does not convert unsupported formats.")
                    }
                } else {
                    NativePlayer(player: model.player).background(.black)
                }
            }
            .navigationTitle(playback.name)
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            #endif
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } } }
        }
        #if os(macOS)
        .frame(minWidth: 800, minHeight: 520)
        #endif
        .onDisappear { model.player.pause() }
    }
}
