import Foundation
import AVFoundation
@main struct Probe {
    @MainActor static func main() async throws {
        let start = Date()
        let asset = AVURLAsset(url: URL(string: "http://127.0.0.1:18769/movie.mp4")!)
        guard let group = try await asset.loadMediaSelectionGroup(for: .legible) else { fatalError("No subtitles") }
        let item = AVPlayerItem(asset: asset)
        let player = AVPlayer(playerItem: item); player.isMuted = true
        for _ in 0..<100 {
            if item.status != .unknown { break }
            try await Task.sleep(for: .milliseconds(100))
        }
        guard item.status == .readyToPlay else { fatalError("Player not ready") }
        print("Ready seconds", Date().timeIntervalSince(start))
        for language in ["en", "zh"] {
            guard let option = group.options.first(where: { ($0.extendedLanguageTag ?? "").hasPrefix(language) && !$0.hasMediaCharacteristic(.containsOnlyForcedSubtitles) }) else { fatalError("Missing language") }
            item.select(option, in: group)
            precondition(item.currentMediaSelection.selectedMediaOption(in: group) == option)
            for seconds in [90.0, 5.0, 115.0, 2.0] {
                let ok = await player.seek(to: CMTime(seconds: seconds, preferredTimescale: 600), toleranceBefore: .zero, toleranceAfter: .zero)
                precondition(ok && abs(player.currentTime().seconds - seconds) < 0.1)
            }
            print(language, "selection and forward/back/end seeks passed")
        }
        item.select(nil, in: group)
        precondition(item.currentMediaSelection.selectedMediaOption(in: group) == nil)
        print("Off selection passed")
        player.replaceCurrentItem(with: nil)
    }
}
