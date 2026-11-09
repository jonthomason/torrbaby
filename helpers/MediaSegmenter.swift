// AVFoundation passthrough packaging for the Rust server's future HLS delivery.
// Development CLI only until worker/caching/session integration is complete.
import Foundation
import AVFoundation
import UniformTypeIdentifiers

struct PackagingError: Error, LocalizedError {
    let message: String
    var errorDescription: String? { message }
}
final class SegmentSink: NSObject, AVAssetWriterDelegate {
    let directory: URL
    private let lock = NSLock()
    private var failure: Error?
    private var segments: [(String, Double)] = []
    private var bytes = 0
    private let maxBytes: Int
    init(_ directory: URL, maxBytes: Int) { self.directory = directory; self.maxBytes = maxBytes }
    var byteCount: Int { lock.lock(); defer { lock.unlock() }; return bytes }
    var error: Error? { lock.lock(); defer { lock.unlock() }; return failure }
    func assetWriter(_ writer: AVAssetWriter, didOutputSegmentData data: Data,
                     segmentType: AVAssetSegmentType, segmentReport: AVAssetSegmentReport?) {
        lock.lock(); defer { lock.unlock() }
        guard failure == nil else { return }
        do {
            bytes += data.count
            guard bytes <= maxBytes else { throw PackagingError(message: "Packaging cache limit exceeded") }
            let name: String
            if segmentType == .initialization {
                name = "init.mp4"
            } else {
                guard let reports = segmentReport?.trackReports,
                      let report = reports.first(where: { $0.mediaType == .video }) ?? reports.first,
                      report.duration.seconds.isFinite, report.duration.seconds > 0 else {
                    throw PackagingError(message: "Missing segment timing")
                }
                name = "segment-\(segments.count).m4s"
                segments.append((name, report.duration.seconds))
            }
            let free = try directory.resourceValues(forKeys: [.volumeAvailableCapacityKey]).volumeAvailableCapacity ?? 0
            guard free > data.count + 1_073_741_824 else { throw PackagingError(message: "Insufficient free disk space") }
            try data.write(to: directory.appendingPathComponent(name), options: .atomic)
        } catch { failure = error }
    }
    func finish() throws {
        lock.lock(); defer { lock.unlock() }
        if let failure { throw failure }
        guard !segments.isEmpty, FileManager.default.fileExists(atPath: directory.appendingPathComponent("init.mp4").path) else {
            throw PackagingError(message: "No media segments produced")
        }
        let target = Int(ceil(segments.map(\.1).max()!))
        var playlist = "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:\(target)\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-MAP:URI=\"init.mp4\"\n"
        for (name, duration) in segments { playlist += String(format: "#EXTINF:%.6f,\n%@\n", duration, name) }
        playlist += "#EXT-X-ENDLIST\n"
        try playlist.write(to: directory.appendingPathComponent("media.m3u8"), atomically: true, encoding: .utf8)
    }
}

@main struct MediaSegmenter {
    static func main() async {
        var stage = "open"
        do {
            guard CommandLine.arguments.count == 4 else { throw PackagingError(message: "Usage: torrbaby-media-segmenter INPUT OUTPUT_DIRECTORY MAX_BYTES") }
            guard let maxBytes = Int(CommandLine.arguments[3]), (1_048_576...34_359_738_368).contains(maxBytes) else {
                throw PackagingError(message: "Invalid packaging budget")
            }
            let input = URL(fileURLWithPath: CommandLine.arguments[1])
            let output = URL(fileURLWithPath: CommandLine.arguments[2], isDirectory: true)
            guard input.isFileURL, FileManager.default.fileExists(atPath: input.path) else { throw PackagingError(message: "Input is unavailable") }
            try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
            guard try FileManager.default.contentsOfDirectory(atPath: output.path).isEmpty else { throw PackagingError(message: "Output directory must be empty") }
            let asset = AVURLAsset(url: input)
            stage = "load tracks"
            let tracks = try await asset.load(.tracks).filter { $0.mediaType == .video || $0.mediaType == .audio }
            guard !tracks.isEmpty, tracks.count <= 8 else { throw PackagingError(message: "Unsupported media tracks") }
            guard tracks.filter({ $0.mediaType == .video }).count <= 1 else { throw PackagingError(message: "Multiple video tracks unsupported") }
            var used = 0
            var renditions: [(String, AVMediaType, String?, Double)] = []
            for (index, track) in tracks.enumerated() {
                stage = "package track \(index + 1)"
                let name = "track-\(index)"
                let directory = output.appendingPathComponent(name, isDirectory: true)
                try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false)
                used += try await packageTrack(asset: asset, track: track, output: directory, maxBytes: maxBytes - used)
                let language = try await track.load(.extendedLanguageTag)
                let rate = try await track.load(.estimatedDataRate)
                renditions.append((name, track.mediaType, language, Double(rate)))
            }
            var master = "#EXTM3U\n#EXT-X-VERSION:7\n"
            let audio = renditions.filter { $0.1 == .audio }
            for (index, rendition) in audio.enumerated() {
                let language = rendition.2.flatMap { value in
                    value.utf8.allSatisfy { ($0 >= 65 && $0 <= 90) || ($0 >= 97 && $0 <= 122) || ($0 >= 48 && $0 <= 57) || $0 == 45 } ? value : nil
                }
                let languageAttribute = language.map { ",LANGUAGE=\"\($0)\"" } ?? ""
                master += "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio\",NAME=\"Audio \(index + 1)\",DEFAULT=\(index == 0 ? "YES" : "NO"),AUTOSELECT=YES\(languageAttribute),URI=\"\(rendition.0)/media.m3u8\"\n"
            }
            guard let primary = renditions.first(where: { $0.1 == .video }) ?? renditions.first else { throw PackagingError(message: "No playable tracks") }
            let rate = primary.3 + (primary.1 == .video ? audio.map(\.3).max() ?? 0 : 0)
            let bandwidth = rate.isFinite && rate > 0 ? Int(min(1_000_000_000, max(128_000, rate * 1.5))) : 20_000_000
            master += "#EXT-X-STREAM-INF:BANDWIDTH=\(bandwidth)\(primary.1 == .video && !audio.isEmpty ? ",AUDIO=\"audio\"" : "")\n\(primary.0)/media.m3u8\n"
            try master.write(to: output.appendingPathComponent("media.m3u8"), atomically: true, encoding: .utf8)
            print("{\"version\":1,\"playlist\":\"media.m3u8\"}")
        } catch {
            // Do not emit media paths or contents in diagnostics.
            var detail: String
            if let failure = error as? PackagingError { detail = failure.message }
            else {
                let failure = error as NSError
                detail = "\(failure.domain) (\(failure.code))"
                if let reason = failure.localizedFailureReason {
                    detail += ": " + reason.replacingOccurrences(of: CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "", with: "[media]")
                }
            }
            FileHandle.standardError.write(Data("Media packaging failed at \(stage): \(detail)\n".utf8))
            exit(1)
        }
    }
    static func packageTrack(asset: AVAsset, track: AVAssetTrack, output: URL, maxBytes: Int) async throws -> Int {
            let reader = try AVAssetReader(asset: asset)
            let writer = AVAssetWriter(contentType: .mpeg4Movie)
            defer {
                if reader.status == .reading { reader.cancelReading() }
                if writer.status == .writing { writer.cancelWriting() }
            }
            writer.outputFileTypeProfile = .mpeg4AppleHLS
            writer.preferredOutputSegmentInterval = CMTime(seconds: 6, preferredTimescale: 600)
            writer.initialSegmentStartTime = .zero
            let sink = SegmentSink(output, maxBytes: maxBytes)
            writer.delegate = sink
            var pairs: [(AVAssetReaderTrackOutput, AVAssetWriterInput)] = []
            for track in [track] {
                let descriptions = try await track.load(.formatDescriptions)
                guard let hint = descriptions.first else { throw PackagingError(message: "Missing format description") }
                let source = AVAssetReaderTrackOutput(track: track, outputSettings: nil)
                source.alwaysCopiesSampleData = false
                let destination = AVAssetWriterInput(mediaType: track.mediaType, outputSettings: nil, sourceFormatHint: hint)
                destination.expectsMediaDataInRealTime = false
                if track.mediaType == .video { destination.transform = try await track.load(.preferredTransform) }
                guard reader.canAdd(source), writer.canAdd(destination) else { throw PackagingError(message: "Unsupported passthrough format") }
                reader.add(source); writer.add(destination); pairs.append((source, destination))
            }
            guard writer.startWriting() else { throw writer.error ?? PackagingError(message: "Cannot start packaging") }
            writer.startSession(atSourceTime: .zero)
            guard reader.startReading() else { throw reader.error ?? PackagingError(message: "Cannot read media") }
            var ended = Set<Int>()
            let deadline = Date().addingTimeInterval(600)
            while ended.count != pairs.count {
                if let error = sink.error { throw error }
                guard Date() < deadline else { throw PackagingError(message: "Packaging timed out") }
                guard writer.status == .writing else { throw writer.error ?? PackagingError(message: "Packaging stopped") }
                var advanced = false
                for (index, pair) in pairs.enumerated() where !ended.contains(index) && pair.1.isReadyForMoreMediaData {
                    if let sample = pair.0.copyNextSampleBuffer() {
                        guard pair.1.append(sample) else { throw writer.error ?? PackagingError(message: "Cannot append media sample") }
                    } else {
                        guard reader.status != .failed else { throw reader.error ?? PackagingError(message: "Media read failed") }
                        pair.1.markAsFinished(); ended.insert(index)
                    }
                    advanced = true
                }
                if !advanced { try await Task.sleep(for: .milliseconds(2)) }
            }
            await writer.finishWriting()
            guard writer.status == .completed else { throw writer.error ?? PackagingError(message: "Packaging failed") }
            try sink.finish()
            return sink.byteCount
    }

}
