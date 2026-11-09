import Foundation
import AVFoundation

final class Captions: NSObject, AVPlayerItemLegibleOutputPushDelegate {
    func legibleOutput(_ output: AVPlayerItemLegibleOutput, didOutputAttributedStrings strings: [NSAttributedString], nativeSampleBuffers: [Any], forItemTime time: CMTime) {
        print("CAPTION", time.seconds, strings.map(\.string))
    }
}
@main struct Probe {
    @MainActor static func main() async throws {
        let start = Date()
        let video = AVURLAsset(url: URL(string: CommandLine.arguments[1])!)
        let captions = AVURLAsset(url: URL(fileURLWithPath: CommandLine.arguments[2]))
        let composition = AVMutableComposition()
        let duration = try await video.load(.duration)
        for source in try await video.load(.tracks) where source.mediaType == .video || source.mediaType == .audio {
            let track = composition.addMutableTrack(withMediaType: source.mediaType, preferredTrackID: kCMPersistentTrackID_Invalid)!
            try track.insertTimeRange(CMTimeRange(start: .zero, duration: duration), of: source, at: .zero)
            track.preferredTransform = try await source.load(.preferredTransform)
        }
        for source in try await captions.load(.tracks) {
            let track = composition.addMutableTrack(withMediaType: source.mediaType, preferredTrackID: kCMPersistentTrackID_Invalid)!
            try track.insertTimeRange(try await source.load(.timeRange), of: source, at: .zero)
            track.languageCode = try await source.load(.languageCode)
            track.extendedLanguageTag = try await source.load(.extendedLanguageTag)
        }
        print("Composition assembled in", Date().timeIntervalSince(start), "seconds")
        for (name, asset) in [("subtitle-only asset", captions as AVAsset), ("composition", composition as AVAsset)] {
            print(name, "playable", try await asset.load(.isPlayable))
            for track in try await asset.load(.tracks) {
                print("track", track.mediaType.rawValue, try await track.load(.languageCode) ?? "nil", try await track.load(.extendedLanguageTag) ?? "nil")
            }
            if let group = try await asset.loadMediaSelectionGroup(for: .legible) {
                print(name, "native subtitle group:", group.options.map { $0.extendedLanguageTag ?? $0.displayName }, "Off:", group.allowsEmptySelection)
            } else { print(name, "NO native subtitle selection group") }
        }
        let item = AVPlayerItem(asset: composition)
        let delegate = Captions()
        let output = AVPlayerItemLegibleOutput()
        output.setDelegate(delegate, queue: .main)
        item.add(output)
        let player = AVPlayer(playerItem: item)
        player.isMuted = true
        for _ in 0..<100 {
            if item.status != .unknown { break }
            try await Task.sleep(for: .milliseconds(100))
        }
        print("Ready:", item.status == .readyToPlay, "elapsed:", Date().timeIntervalSince(start), "error:", item.error?.localizedDescription ?? "none")
        for seconds in [1.0, 6.0, 2.0] {
            let ok = await player.seek(to: CMTime(seconds: seconds, preferredTimescale: 600), toleranceBefore: .zero, toleranceAfter: .zero)
            print("Seek", seconds, ok, "position", player.currentTime().seconds)
        }
        for language in ["eng", "zho", "off"] {
            for track in item.tracks {
                if let assetTrack = track.assetTrack, assetTrack.mediaType == .subtitle || assetTrack.mediaType == .text {
                    track.isEnabled = try await assetTrack.load(.languageCode) == language
                    print("Enabled", try await assetTrack.load(.languageCode) ?? "nil", track.isEnabled)
                }
            }
            _ = await player.seek(to: CMTime(seconds: 0, preferredTimescale: 600), toleranceBefore: .zero, toleranceAfter: .zero)
            print("Play language", language)
            player.play()
            try await Task.sleep(for: .seconds(2))
            player.pause()
        }
        withExtendedLifetime(delegate) {}
        player.replaceCurrentItem(with: nil)
    }
}
