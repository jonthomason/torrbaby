import Foundation
import AVFoundation
import CoreMedia
import Speech
import NaturalLanguage

// Transcribes a title's spoken English into timed subtitle cues with Apple's
// on-device SpeechAnalyzer. The server calls it only for files with no usable
// English or Chinese subtitles, then translates the result like any other text
// track. Nothing leaves the Mac: audio is decoded locally and the transcription
// model runs on device (the system installs the English model once).
//
// Arguments: media path or loopback URL, result path, progress path.
// Exit 0 writes the result; exit 3 means the file has no decodable audio;
// any other failure exits 1 with a short reason on stderr.

struct TextCue: Codable { let start_ms: Int; let end_ms: Int; let text: String }
struct Output: Codable {
    let language: String
    let method: String
    /// Mean word confidence (0–1). Speech in another language transcribed as
    /// English scores low; the server uses it to reject such tracks.
    let confidence: Double
    let words: Int
    let speech_seconds: Double
    let duration_seconds: Double
    let detected_language: String
    let cues: [TextCue]
}
struct TranscriptionFailure: Error { let message: String; let status: Int32 }

/// Pull-based audio input: the analyzer requests the next buffer only when it is
/// ready, so a feature film is never decoded into memory ahead of analysis.
final class AudioInput: AsyncSequence, @unchecked Sendable {
    typealias Element = AnalyzerInput
    let reader: AVAssetReader
    let output: AVAssetReaderTrackOutput
    let format: AVAudioFormat
    let duration: Double
    let progress: URL
    var lastReport = -60.0
    init(reader: AVAssetReader, output: AVAssetReaderTrackOutput, format: AVAudioFormat, duration: Double, progress: URL) {
        self.reader = reader; self.output = output; self.format = format; self.duration = duration; self.progress = progress
    }
    struct Iterator: AsyncIteratorProtocol {
        let input: AudioInput
        mutating func next() async throws -> AnalyzerInput? { try input.nextInput() }
    }
    func makeAsyncIterator() -> Iterator { Iterator(input: self) }

    func nextInput() throws -> AnalyzerInput? {
        while true {
            guard let sample = output.copyNextSampleBuffer() else {
                if reader.status == .failed { throw TranscriptionFailure(message: "Audio decoding failed", status: 1) }
                report(duration)
                return nil
            }
            let frames = CMSampleBufferGetNumSamples(sample)
            if frames <= 0 { continue }
            guard let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(frames)) else {
                throw TranscriptionFailure(message: "Audio buffer allocation failed", status: 1)
            }
            buffer.frameLength = AVAudioFrameCount(frames)
            let status = CMSampleBufferCopyPCMDataIntoAudioBufferList(sample, at: 0, frameCount: Int32(frames), into: buffer.mutableAudioBufferList)
            guard status == noErr else { throw TranscriptionFailure(message: "Audio conversion failed", status: 1) }
            let start = CMSampleBufferGetPresentationTimeStamp(sample)
            report(start.seconds)
            return AnalyzerInput(buffer: buffer, bufferStartTime: start.isNumeric ? start : nil)
        }
    }
    func report(_ seconds: Double) {
        guard seconds.isFinite, seconds - lastReport >= 30 || seconds >= duration else { return }
        lastReport = seconds
        let body: [String: Any] = ["stage": "transcription", "completed_cues": Int(Swift.min(seconds, duration)), "total_cues": Int(duration.rounded(.up))]
        if let data = try? JSONSerialization.data(withJSONObject: body) { try? data.write(to: progress, options: .atomic) }
    }
}

struct Word { var text: String; let start: Double; var end: Double; let confidence: Double? }

/// Subtitle-sized cues from timed words: at most two lines of about 42
/// characters, at most 6 seconds, broken at pauses and sentence ends.
func cues(from words: [Word]) -> [TextCue] {
    var groups: [[Word]] = []
    var current: [Word] = []
    func length(_ ws: [Word]) -> Int { ws.map(\.text).joined(separator: " ").count }
    for word in words {
        if let last = current.last {
            let gap = word.start - last.end
            let sentenceEnd = last.text.last.map { ".?!".contains($0) } ?? false
            let clauseEnd = last.text.last.map { ",;:".contains($0) } ?? false
            let size = length(current)
            let joined = length(current + [word])
            if joined > 84 || word.end - current[0].start > 6.0 {
                // Full: end the cue at the last punctuation in its second half, so
                // a clause is not split across cues just because the cue filled up.
                let marks = current.indices.filter { i in i + 1 < current.count && i + 1 >= current.count / 2
                    && current[i].text.last.map { ",.;:?!".contains($0) } == true }
                let cut = (marks.last ?? current.count - 1) + 1
                groups.append(Array(current[..<cut])); current = Array(current[cut...])
            } else if gap > 0.8 || (sentenceEnd && size >= 12 && (gap > 0.3 || joined > 42)) || (clauseEnd && size >= 60) {
                // Short consecutive sentences spoken together share a cue.
                groups.append(current); current = []
            }
        }
        current.append(word)
    }
    if !current.isEmpty { groups.append(current) }
    var result: [TextCue] = []
    for (index, group) in groups.enumerated() {
        let start = group[0].start
        let next = index + 1 < groups.count ? groups[index + 1][0].start : .infinity
        // Linger briefly after the last word, and keep short cues readable, but
        // never overlap the next cue.
        var end = max(group.last!.end + 0.4, start + 1.0)
        end = min(end, next - 0.05)
        if end <= start + 0.2 { end = min(group.last!.end, next) }
        guard end > start else { continue }
        result.append(TextCue(start_ms: Int((start * 1000).rounded()), end_ms: Int((end * 1000).rounded()), text: lines(group.map(\.text))))
    }
    return result
}

/// One line if it fits, otherwise two lines balanced at a word boundary,
/// preferring a break after punctuation.
func lines(_ words: [String]) -> String {
    let text = words.joined(separator: " ")
    guard text.count > 42, words.count > 1 else { return text }
    var best = 1; var bestScore = Int.max
    for split in 1..<words.count {
        let first = words[..<split].joined(separator: " ").count
        let second = words[split...].joined(separator: " ").count
        var score = abs(first - second)
        if let mark = words[split - 1].last, ",.;:?!".contains(mark) { score -= 8 }
        if max(first, second) > 46 { score += 100 }
        if score < bestScore { bestScore = score; best = split }
    }
    return words[..<best].joined(separator: " ") + "\n" + words[best...].joined(separator: " ")
}

func transcribe(input: URL, output: URL, progress: URL) async throws {
    let asset = AVURLAsset(url: input)
    let tracks = try await asset.loadTracks(withMediaType: .audio)
    var chosen: AVAssetTrack?
    for track in tracks where chosen == nil {
        let code = (try? await track.load(.languageCode))?.lowercased() ?? ""
        if code == "en" || code == "eng" || code.hasPrefix("en-") { chosen = track }
    }
    guard let track = chosen ?? tracks.first else { throw TranscriptionFailure(message: "No decodable audio track", status: 3) }
    let duration = try await asset.load(.duration).seconds
    guard duration.isFinite, duration > 0, duration < 6 * 3600 else { throw TranscriptionFailure(message: "Invalid media duration", status: 1) }

    guard SpeechTranscriber.isAvailable,
          let locale = await SpeechTranscriber.supportedLocale(equivalentTo: Locale(identifier: "en-US"))
    else { throw TranscriptionFailure(message: "Speech transcription is unavailable on this Mac", status: 1) }
    let transcriber = SpeechTranscriber(locale: locale, transcriptionOptions: [], reportingOptions: [],
                                        attributeOptions: [.audioTimeRange, .transcriptionConfidence])
    if await AssetInventory.status(forModules: [transcriber]) != .installed {
        if let request = try await AssetInventory.assetInstallationRequest(supporting: [transcriber]) {
            try await request.downloadAndInstall()
        }
    }
    guard let analysisFormat = await SpeechAnalyzer.bestAvailableAudioFormat(compatibleWith: [transcriber]) else {
        throw TranscriptionFailure(message: "No audio format for speech transcription", status: 1)
    }
    // Decode straight to the analyzer's mono PCM format; AVFoundation downmixes
    // surround tracks (dialogue lives in the centre channel) and resamples.
    let integer = analysisFormat.commonFormat == .pcmFormatInt16
    guard let format = AVAudioFormat(commonFormat: integer ? .pcmFormatInt16 : .pcmFormatFloat32,
                                     sampleRate: analysisFormat.sampleRate, channels: 1, interleaved: true)
    else { throw TranscriptionFailure(message: "Invalid speech audio format", status: 1) }
    let reader = try AVAssetReader(asset: asset)
    let trackOutput = AVAssetReaderTrackOutput(track: track, outputSettings: [
        AVFormatIDKey: kAudioFormatLinearPCM, AVSampleRateKey: format.sampleRate, AVNumberOfChannelsKey: 1,
        AVLinearPCMBitDepthKey: integer ? 16 : 32, AVLinearPCMIsFloatKey: !integer,
        AVLinearPCMIsBigEndianKey: false, AVLinearPCMIsNonInterleaved: false])
    guard reader.canAdd(trackOutput) else { throw TranscriptionFailure(message: "No decodable audio track", status: 3) }
    reader.add(trackOutput)
    guard reader.startReading() else { throw TranscriptionFailure(message: "No decodable audio track", status: 3) }

    let analyzer = SpeechAnalyzer(modules: [transcriber], options: .init(priority: .utility, modelRetention: .whileInUse))
    let collector = Task { () throws -> [Word] in
        var words: [Word] = []
        for try await result in transcriber.results {
            let text = result.text
            for run in text.runs {
                let piece = String(text[run.range].characters).trimmingCharacters(in: .whitespacesAndNewlines)
                if piece.isEmpty { continue }
                if let range = run.audioTimeRange, range.start.isNumeric, range.duration.isNumeric {
                    let start = range.start.seconds
                    // A run is usually one word; split defensively on spaces.
                    for part in piece.split(separator: " ") {
                        words.append(Word(text: String(part), start: start, end: range.end.seconds, confidence: run.transcriptionConfidence))
                    }
                } else if !words.isEmpty {
                    // Untimed punctuation attaches to the previous word.
                    words[words.count - 1].text += piece
                }
            }
        }
        return words
    }
    try await analyzer.prepareToAnalyze(in: format)
    let input = AudioInput(reader: reader, output: trackOutput, format: format, duration: duration, progress: progress)
    if let last = try await analyzer.analyzeSequence(input) {
        try await analyzer.finalizeAndFinish(through: last)
    } else {
        await analyzer.cancelAndFinishNow()
    }
    let words = try await collector.value.sorted { $0.start < $1.start }
    let result = cues(from: words)
    guard !result.isEmpty else { throw TranscriptionFailure(message: "No speech was recognized", status: 1) }
    let confidences = words.compactMap(\.confidence)
    let recognizer = NLLanguageRecognizer()
    recognizer.processString(String(result.map(\.text).joined(separator: " ").prefix(30000)))
    let speech = words.reduce(0.0) { $0 + max(0, $1.end - $1.start) }
    let document = Output(language: "en", method: "speech",
                          confidence: confidences.isEmpty ? 0 : confidences.reduce(0, +) / Double(confidences.count),
                          words: words.count, speech_seconds: speech, duration_seconds: duration,
                          detected_language: recognizer.dominantLanguage?.rawValue ?? "", cues: result)
    let data = try JSONEncoder().encode(document)
    guard data.count <= 16_777_216 else { throw TranscriptionFailure(message: "Transcript too large", status: 1) }
    try data.write(to: output, options: .atomic)
}

@main struct SpeechSubtitles {
    static func main() async {
        let args = CommandLine.arguments
        guard args.count == 4 else {
            FileHandle.standardError.write(Data("Expected media, result path, progress path\n".utf8)); exit(2)
        }
        let input = args[1].hasPrefix("http://127.0.0.1:") ? URL(string: args[1]) : URL(fileURLWithPath: args[1])
        guard let input else { FileHandle.standardError.write(Data("Invalid media URL\n".utf8)); exit(2) }
        do {
            try await transcribe(input: input, output: URL(fileURLWithPath: args[2]), progress: URL(fileURLWithPath: args[3]))
        } catch let failure as TranscriptionFailure {
            FileHandle.standardError.write(Data("\(failure.message)\n".utf8)); exit(failure.status)
        } catch {
            // Never log transcript text or media paths.
            FileHandle.standardError.write(Data("Speech transcription failed: \((error as NSError).domain) \((error as NSError).code)\n".utf8)); exit(1)
        }
    }
}
