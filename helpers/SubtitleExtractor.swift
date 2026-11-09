// Text-only MP4/QuickTime subtitle extraction. No OCR or media decoding.
import Foundation
import AVFoundation
import NaturalLanguage
struct ExtractionFailure: Error { let message: String }
struct TimedText: Codable { let start_ms: UInt64; let end_ms: UInt64; let text: String }
@main struct SubtitleExtractor {
    static func main() async {
        do {
            if CommandLine.arguments.dropFirst().first == "--detect-language" {
                let data = try FileHandle.standardInput.readToEnd() ?? Data()
                guard data.count <= 32768, let text = String(data: data, encoding: .utf8), text.count >= 200 else {
                    FileHandle.standardOutput.write(Data("null".utf8)); return
                }
                let recognizer = NLLanguageRecognizer()
                recognizer.processString(text)
                let best = recognizer.languageHypotheses(withMaximum: 2).sorted { $0.value > $1.value }
                let language = best.first.flatMap { $0.value >= 0.75 && $0.value - (best.dropFirst().first?.value ?? 0) >= 0.5 ? $0.key.rawValue : nil }
                FileHandle.standardOutput.write(try JSONSerialization.data(withJSONObject: language as Any? ?? NSNull(), options: [.fragmentsAllowed]))
                return
            }
            guard (2...3).contains(CommandLine.arguments.count) else { throw ExtractionFailure(message: "Expected input and optional track ID") }
            let asset = AVURLAsset(url: URL(fileURLWithPath: CommandLine.arguments[1]))
            let allTracks = try await asset.load(.tracks)
            let tracks = allTracks.filter { $0.mediaType == .subtitle || $0.mediaType == .text || $0.mediaType == .closedCaption }
            guard tracks.count <= 256 else { throw ExtractionFailure(message: "Too many subtitle tracks") }
            if CommandLine.arguments.count == 2 {
                var entries: [[String: Any]] = []
                for track in tracks {
                    let formats = try await track.load(.formatDescriptions)
                    let supported = !formats.isEmpty && formats.allSatisfy { CMFormatDescriptionGetMediaSubType($0) == 0x74783367 }
                    let tag = try await track.load(.extendedLanguageTag)
                    let code = try await track.load(.languageCode)
                    let language = tag ?? code
                    entries.append(["track": track.trackID, "codec": supported ? "tx3g" : "unsupported", "language": language as Any? ?? NSNull(), "name": NSNull(), "forced": false, "hearing_impaired": false, "text_extractable": supported, "extraction_supported": supported, "encoded": false])
                }
                var audio: [[String: Any]] = []
                for track in allTracks where track.mediaType == .audio {
                    let tag = try await track.load(.extendedLanguageTag)
                    let code = try await track.load(.languageCode)
                    let formats = try await track.load(.formatDescriptions)
                    let format = formats.first.map { CMFormatDescriptionGetMediaSubType($0) } ?? 0
                    let codec = String(bytes: [UInt8((format >> 24) & 255), UInt8((format >> 16) & 255), UInt8((format >> 8) & 255), UInt8(format & 255)], encoding: .ascii) ?? "unknown"
                    audio.append(["track": track.trackID, "codec": codec, "language": (tag ?? code) as Any? ?? NSNull()])
                }
                FileHandle.standardOutput.write(try JSONSerialization.data(withJSONObject: ["container": "mp4", "tracks": entries, "audio_tracks": audio]))
            } else {
                guard let id = Int32(CommandLine.arguments[2]), let track = tracks.first(where: { $0.trackID == id }) else { throw ExtractionFailure(message: "Subtitle track missing") }
                let formats = try await track.load(.formatDescriptions)
                guard !formats.isEmpty, formats.allSatisfy({ CMFormatDescriptionGetMediaSubType($0) == 0x74783367 }) else { throw ExtractionFailure(message: "Only tx3g text subtitles are supported") }
                let reader = try AVAssetReader(asset: asset)
                defer { if reader.status == .reading { reader.cancelReading() } }
                let output = AVAssetReaderTrackOutput(track: track, outputSettings: nil)
                guard reader.canAdd(output) else { throw ExtractionFailure(message: "Cannot read subtitle track") }
                reader.add(output)
                guard reader.startReading() else { throw ExtractionFailure(message: "Cannot start subtitle extraction") }
                var cues: [TimedText] = []
                var bytes = 0
                let deadline = Date().addingTimeInterval(120)
                while let batch = output.copyNextSampleBuffer() {
                    let count = CMSampleBufferGetNumSamples(batch)
                    guard count <= 100_000 else { throw ExtractionFailure(message: "Subtitle sample batch too large") }
                    for index in 0..<count {
                    var copied: CMSampleBuffer?
                    let result = CMSampleBufferCopySampleBufferForRange(allocator: kCFAllocatorDefault, sampleBuffer: batch, sampleRange: CFRange(location: index, length: 1), sampleBufferOut: &copied)
                    guard result == noErr, let sample = copied else { throw ExtractionFailure(message: "Cannot split subtitle samples") }
                    guard Date() < deadline, cues.count < 100_000 else { throw ExtractionFailure(message: "Subtitle extraction limit exceeded") }
                    guard CMSampleBufferGetNumSamples(sample) == 1, let block = CMSampleBufferGetDataBuffer(sample) else { throw ExtractionFailure(message: "Unsupported subtitle sample layout") }
                    let length = CMBlockBufferGetDataLength(block)
                    guard length >= 2, length <= 1_048_576 else { throw ExtractionFailure(message: "Invalid subtitle sample size") }
                    var data = Data(count: length)
                    let status = data.withUnsafeMutableBytes { CMBlockBufferCopyDataBytes(block, atOffset: 0, dataLength: length, destination: $0.baseAddress!) }
                    guard status == kCMBlockBufferNoErr else { throw ExtractionFailure(message: "Cannot read subtitle sample") }
                    let size = Int(data[0]) * 256 + Int(data[1])
                    guard size <= length - 2 else { throw ExtractionFailure(message: "Invalid subtitle text length") }
                    if size == 0 { continue }
                    let payload = Data(data[2..<(size + 2)])
                    let utf16 = payload.starts(with: [0xfe, 0xff]) || payload.starts(with: [0xff, 0xfe])
                    guard let text = String(data: payload, encoding: utf16 ? .utf16 : .utf8) else { throw ExtractionFailure(message: "Invalid subtitle text encoding") }
                    let clean = text.replacingOccurrences(of: "\r\n", with: "\n").replacingOccurrences(of: "\r", with: "\n").trimmingCharacters(in: .whitespacesAndNewlines)
                    if clean.isEmpty { continue }
                    let start = CMSampleBufferGetPresentationTimeStamp(sample).seconds
                    let duration = CMSampleBufferGetDuration(sample).seconds
                    guard start.isFinite, duration.isFinite, start >= 0, duration > 0, start + duration < 604800 else { throw ExtractionFailure(message: "Invalid subtitle timing") }
                    bytes += clean.utf8.count
                    guard bytes <= 8_388_608 else { throw ExtractionFailure(message: "Subtitle text limit exceeded") }
                    cues.append(TimedText(start_ms: UInt64((start * 1000).rounded()), end_ms: UInt64(((start + duration) * 1000).rounded()), text: clean))
                    }
                }
                guard reader.status == .completed else { throw ExtractionFailure(message: "Subtitle extraction failed") }
                cues.sort { $0.start_ms < $1.start_ms }
                let data = try JSONEncoder().encode(cues)
                guard data.count <= 16_777_216 else { throw ExtractionFailure(message: "Subtitle output limit exceeded") }
                FileHandle.standardOutput.write(data)
            }
        } catch {
            let message = (error as? ExtractionFailure)?.message ?? "Native subtitle reader failed"
            FileHandle.standardError.write(Data((message + "\n").utf8)); exit(1)
        }
    }
}
