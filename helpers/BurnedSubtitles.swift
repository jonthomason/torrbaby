import Foundation
import Vision
import CoreGraphics
import CoreText

// Reads the subtitles printed in a title's picture. Chinese dramas and films are
// usually distributed with their Chinese subtitles part of the image and no
// subtitle track at all; the text is exact where a transcript of the speech
// mishears names and period language. The bundled ffmpeg decodes the video (on
// the Mac's video hardware) and hands over a few grey frames a second of the
// strip the subtitles sit in; Vision reads each strip that changed, and lines
// that hold still become timed cues.
//
// A short first pass over three stretches of the title finds whether there are
// such subtitles and where they sit, so a title without them costs seconds.
//
// Arguments: media path, result path, progress path, and optionally the folder
// holding torrbaby-ffmpeg and torrbaby-ffprobe (this helper's own by default).
// Exit 0 writes the result; exit 4 means the picture carries no subtitles;
// exit 3 that the video could not be decoded; 75 that the text models changed
// under this job (the server tries again). Other failures exit 1.

// Vision's text recognizer always uses Neural Engine models. Requesting another
// compute device (setComputeDevice(.cpu), usesCPUOnly) still compiles and loads
// them through the system ANECompilerService. Compiled models are cached for
// the whole server bundle, but concurrent processes do not share a compile that
// is still running: each one compiles the same models again and the system
// compiler saturates. Vision also loads the compiled program again for every
// image. When one process recompiles, the program file that other running
// helpers use is replaced and their next image fails (CRImageReaderError, e5rt
// code 13). Retrying in such a process recompiles again and breaks the others
// in turn. So:
// - Helpers compile only under one cross-process lock. The warm-up recognizes
//   real Latin and Chinese text with every request setting the helpers use, so
//   detection and all recognizers load (and recompile if needed) while the
//   lock is held.
// - After the warm-up a recognition failure ends the job without retrying. The
//   server requeues jobs whose error contains textModelsChanged; a new process
//   then warms up under the lock and finds the program the last compile left.
// Keep this block identical in SubtitleOCR.swift and CreditsScanner.swift.
let textModelsChanged = "Text recognition models changed; retry later"
func textWarmUpImage() -> CGImage? {
    let width = 960, height = 120
    guard let c = CGContext(data:nil,width:width,height:height,bitsPerComponent:8,bytesPerRow:0,
                            space:CGColorSpaceCreateDeviceGray(),bitmapInfo:CGImageAlphaInfo.none.rawValue) else { return nil }
    c.setFillColor(gray:1,alpha:1); c.fill(CGRect(x:0,y:0,width:width,height:height))
    let text = NSAttributedString(string:"Warm up 2026 字幕测试",attributes:[
        NSAttributedString.Key(kCTFontAttributeName as String):CTFontCreateWithName("PingFang SC" as CFString,44,nil),
        NSAttributedString.Key(kCTForegroundColorAttributeName as String):CGColor(gray:0,alpha:1)])
    c.textPosition = CGPoint(x:24,y:40); CTLineDraw(CTLineCreateWithAttributedString(text),c)
    return c.makeImage()
}
func prepareTextRecognition() throws {
    let caches = FileManager.default.urls(for:.cachesDirectory,in:.userDomainMask)[0]
        .appendingPathComponent("org.torrbaby.server",isDirectory:true)
    try FileManager.default.createDirectory(at:caches,withIntermediateDirectories:true)
    let fd = open(caches.appendingPathComponent("text-recognition.lock").path,O_RDWR|O_CREAT|O_CLOEXEC,0o600)
    guard fd >= 0, flock(fd,LOCK_EX) == 0 else { if fd >= 0 { close(fd) }; throw NSError(domain:"Text recognition lock unavailable",code:1) }
    defer { flock(fd,LOCK_UN); close(fd) }
    guard let image = textWarmUpImage() else { throw NSError(domain:"Text recognition warm-up failed",code:1) }
    var failure: Error = NSError(domain:"Text recognition warm-up read no text",code:1)
    for attempt in 0..<3 {
        // A failed load makes Vision recompile on the next attempt; a compile that
        // timed out on the client side usually completes in the system service.
        if attempt > 0 { Thread.sleep(forTimeInterval:5) }
        do {
            var read = true
            // Every request the helpers make, including OCR's fast English fallback.
            let settings: [(VNRequestTextRecognitionLevel,[String],Bool)] = [(.accurate,["en-US","zh-Hans"],false),
                (.accurate,["en-US"],true),(.accurate,["zh-Hans"],true),(.fast,["en-US"],true)]
            for (level,languages,correction) in settings {
                let request = VNRecognizeTextRequest()
                request.recognitionLevel = level; request.recognitionLanguages = languages
                request.usesLanguageCorrection = correction
                try VNImageRequestHandler(cgImage:image,options:[:]).perform([request])
                read = read && !(request.results ?? []).isEmpty
            }
            if read { return }
        } catch { failure = error }
    }
    throw failure
}

// Exit status the server maps to a requeue rather than a failure.
let modelsChangedStatus: Int32 = 75
func modelsChanged(_ detail:String) -> Never {
 FileHandle.standardError.write(Data("\(textModelsChanged)\(detail)\n".utf8)); exit(modelsChangedStatus)
}
func recognize(_ request:VNRecognizeTextRequest,_ image:CGImage) {
 do {try VNImageRequestHandler(cgImage:image,options:[:]).perform([request])} catch {modelsChanged(" (\(error))")}
}
struct TextCue: Codable { let start_ms: Int; let end_ms: Int; let text: String }
struct Output: Codable {
    let language: String
    let method: String
    let characters: Int
    let cue_seconds: Double
    let duration_seconds: Double
    let cues: [TextCue]
}
struct Failure: Error { let message: String; let status: Int32 }
struct Line { let text: String; let midX: Double; let midY: Double; let height: Double }

let sampleRate = 4.0
/// The part of the picture searched for subtitles in the first pass: its lower third.
let searchTop = 0.62, searchBottom = 0.98

func ideographs(_ text: String) -> Int { text.unicodeScalars.filter { (0x4E00...0x9FFF).contains($0.value) }.count }

func tool(_ name: String, _ folder: URL) -> URL { folder.appendingPathComponent(name) }

/// Width and height of each video stream, in order, and the title's duration. Some
/// files open with a stream that holds no picture (a cover, a stub in another codec).
func describe(_ media: String, _ tools: URL) throws -> (streams: [(width: Int, height: Int)], duration: Double) {
    let process = Process(); let pipe = Pipe()
    process.executableURL = tool("torrbaby-ffprobe", tools)
    process.arguments = ["-v", "error", "-select_streams", "v", "-show_entries", "stream=width,height:format=duration", "-of", "default=noprint_wrappers=1", media]
    process.standardOutput = pipe; process.standardError = FileHandle.nullDevice; process.standardInput = FileHandle.nullDevice
    try process.run()
    let text = String(decoding: pipe.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
    process.waitUntilExit()
    var widths: [Int] = [], heights: [Int] = [], duration = 0.0
    for line in text.split(separator: "\n") {
        let parts = line.split(separator: "=", maxSplits: 1)
        guard parts.count == 2, let value = Double(parts[1]) else { continue }
        switch parts[0] { case "width": widths.append(Int(value)); case "height": heights.append(Int(value)); case "duration": duration = value; default: break }
    }
    let streams = zip(widths, heights).map { (width: $0, height: $1) }
    guard !streams.isEmpty, streams.count <= 8, duration > 0, duration < 12 * 3600 else { throw Failure(message: "No decodable video", status: 3) }
    return (streams, duration)
}

/// Grey frames of one horizontal strip of the picture (`top` to `bottom`, as fractions
/// of its height), `rate` a second, each `width` by `height` bytes, handed to `each`
/// with its index. `each` returns false to stop early.
func frames(_ media: String, _ tools: URL, stream: Int, start: Double, length: Double?, top: Double, bottom: Double, rate: Double,
            width: Int, height: Int, each: (Int, Data) -> Bool) throws {
    let process = Process(); let pipe = Pipe()
    process.executableURL = tool("torrbaby-ffmpeg", tools)
    var arguments = ["-v", "error", "-nostdin", "-hwaccel", "videotoolbox", "-ss", String(format: "%.3f", start)]
    if let length { arguments += ["-t", String(format: "%.3f", length)] }
    let crop = String(format: "crop=iw:ih*%.4f:0:ih*%.4f", bottom - top, top)
    arguments += ["-i", media, "-map", "0:v:\(stream)", "-an", "-sn", "-dn",
                  "-vf", "fps=\(rate),\(crop),scale=\(width):\(height),format=gray", "-f", "rawvideo", "-"]
    process.arguments = arguments
    process.standardOutput = pipe; process.standardError = FileHandle.nullDevice; process.standardInput = FileHandle.nullDevice
    try process.run()
    defer { if process.isRunning { process.terminate() }; process.waitUntilExit() }
    let size = width * height, reader = pipe.fileHandleForReading
    var index = 0
    while true {
        var data = Data(capacity: size)
        while data.count < size { let chunk = reader.readData(ofLength: size - data.count); if chunk.isEmpty { break }; data.append(chunk) }
        guard data.count == size else { break }
        if !autoreleasepool(invoking: { each(index, data) }) { break }
        index += 1
    }
}

func read(_ data: Data, _ width: Int, _ height: Int) -> [Line] {
    guard let provider = CGDataProvider(data: data as CFData),
          let image = CGImage(width: width, height: height, bitsPerComponent: 8, bitsPerPixel: 8, bytesPerRow: width,
                              space: CGColorSpaceCreateDeviceGray(), bitmapInfo: CGBitmapInfo(rawValue: 0), provider: provider,
                              decode: nil, shouldInterpolate: false, intent: .defaultIntent) else { return [] }
    let request = VNRecognizeTextRequest()
    request.recognitionLevel = .accurate; request.recognitionLanguages = ["zh-Hans"]; request.usesLanguageCorrection = true
    recognize(request, image)
    return (request.results ?? []).compactMap { observation in
        guard let best = observation.topCandidates(1).first, best.confidence >= 0.3 else { return nil }
        let box = observation.boundingBox
        return Line(text: best.string, midX: box.midX, midY: box.midY, height: box.height)
    }
}

/// A coarse picture of the strip: the mean of each 8 by 8 block. Two strips whose
/// pictures barely differ show the same subtitle (or none), so only one is read.
func coarse(_ data: Data, _ width: Int, _ height: Int) -> [UInt8] {
    var out = [UInt8](); out.reserveCapacity((width / 8) * (height / 8))
    data.withUnsafeBytes { (bytes: UnsafeRawBufferPointer) in
        var y = 0
        while y + 8 <= height {
            var x = 0
            while x + 8 <= width {
                var sum = 0
                for row in stride(from: y, to: y + 8, by: 2) { for column in stride(from: x, to: x + 8, by: 2) { sum += Int(bytes[row * width + column]) } }
                out.append(UInt8(sum / 16)); x += 8
            }
            y += 8
        }
    }
    return out
}
func same(_ a: [UInt8], _ b: [UInt8]) -> Bool {
    guard a.count == b.count, !a.isEmpty else { return false }
    var changed = 0
    for i in 0..<a.count where abs(Int(a[i]) - Int(b[i])) > 18 { changed += 1 }
    return changed * 150 < a.count
}

/// How many single-character edits separate two readings, up to `limit`.
func distance(_ a: [Character], _ b: [Character], limit: Int) -> Int {
    if abs(a.count - b.count) > limit { return limit + 1 }
    var previous = Array(0...b.count)
    for i in 0..<a.count {
        var current = [i + 1] + Array(repeating: 0, count: b.count)
        for j in 0..<b.count { current[j + 1] = min(previous[j] + (a[i] == b[j] ? 0 : 1), previous[j + 1] + 1, current[j] + 1) }
        previous = current
    }
    return previous[b.count]
}
/// Two readings of one subtitle: the same but for a character or two Vision read differently.
func alike(_ a: String, _ b: String) -> Bool {
    if a == b { return true }
    let (x, y) = (Array(a), Array(b)), limit = max(1, min(x.count, y.count) / 4)
    return distance(x, y, limit: limit) <= limit
}
/// Subtitles in the picture separate phrases with a space where a sentence would have a comma.
func punctuated(_ text: String) -> String {
    // A speck beside the line is sometimes read as a quotation mark or a bar.
    let characters = Array(text.trimmingCharacters(in: CharacterSet(charactersIn: " “”\"'‘’|｜*·.")))
    var out = ""
    for (i, c) in characters.enumerated() {
        if c == " ", i > 0, i + 1 < characters.count, ideographs(String(characters[i - 1])) == 1, ideographs(String(characters[i + 1])) == 1 { out.append("，") }
        else { out.append(c) }
    }
    return out
}

/// Cues from what each sampled strip showed ("" for nothing): a line must hold for two
/// samples, a reading dropped for a sample or two in the middle of a line does not end it,
/// and the commonest reading of a line is the one kept.
func cues(from shown: [String], rate: Double) -> [TextCue] {
    struct Run { var first: Int; var last: Int; var readings: [String: Int] }
    var runs: [Run] = []
    for (index, text) in shown.enumerated() where !text.isEmpty {
        if var run = runs.last, index - run.last <= 3, run.readings.keys.contains(where: { alike($0, text) }) {
            run.last = index; run.readings[text, default: 0] += 1; runs[runs.count - 1] = run
        } else { runs.append(Run(first: index, last: index, readings: [text: 1])) }
    }
    var result: [TextCue] = []
    for (i, run) in runs.enumerated() where run.last > run.first {
        guard let text = run.readings.max(by: { ($0.value, $0.key.count) < ($1.value, $1.key.count) })?.key else { continue }
        var end = Double(run.last + 1) / rate
        if i + 1 < runs.count { end = min(end, Double(runs[i + 1].first) / rate) }
        let start = Double(run.first) / rate
        // A sponsor's card before the episode is centred text too.
        guard end > start, !text.contains("邀您观看") else { continue }
        result.append(TextCue(start_ms: Int((start * 1000).rounded()), end_ms: Int((end * 1000).rounded()), text: punctuated(text)))
    }
    return result
}

func extract(media: String, output: URL, progress: URL, tools: URL) throws {
    let (streams, duration) = try describe(media, tools)
    do { try prepareTextRecognition() } catch { modelsChanged(" (warm-up: \(error))") }
    let even = { (value: Double) -> Int in max(16, Int(value / 2) * 2) }
    let width = 1280
    // First pass: is there a line of Chinese, centred, at one height, in the lower third?
    // The picture is the first video stream that decodes.
    var found: [Line] = [], decoded = 0, stream = 0, videoWidth = 0, videoHeight = 0, searchHeight = 0
    for (index, size) in streams.enumerated() where decoded == 0 && size.width >= 320 && size.height >= 240 {
      (stream, videoWidth, videoHeight) = (index, size.width, size.height)
      searchHeight = even(Double(width) * (searchBottom - searchTop) * Double(videoHeight) / Double(videoWidth))
      for place in [0.3, 0.5, 0.7] {
        try frames(media, tools, stream: stream, start: duration * place, length: min(40, duration / 4), top: searchTop, bottom: searchBottom, rate: 2,
                   width: width, height: searchHeight) { _, data in
            decoded += 1
            found += read(data, width, searchHeight).filter { abs($0.midX - 0.5) < 0.07 && ideographs($0.text) >= 2 }
            return true
        }
      }
    }
    guard decoded > 0 else { throw Failure(message: "No decodable video", status: 3) }
    var bins: [Int: [Line]] = [:]
    for line in found { bins[Int(line.midY / 0.04), default: []].append(line) }
    guard let (bin, _) = bins.max(by: { $0.value.count < $1.value.count }) else { throw Failure(message: "No subtitles in the picture", status: 4) }
    let row = (bins[bin - 1] ?? []) + (bins[bin] ?? []) + (bins[bin + 1] ?? [])
    guard Set(row.map(\.text)).count >= 6 else { throw Failure(message: "No subtitles in the picture", status: 4) }
    let middle = row.map(\.midY).sorted()[row.count / 2], tall = row.map(\.height).sorted()[row.count / 2]
    // As fractions of the whole picture, measured from its top.
    let span = searchBottom - searchTop
    let centre = searchTop + (1 - middle) * span, line = tall * span
    // The strip read in the second pass: the subtitle row, with room above for a second row.
    let top = max(0, centre - 2.3 * line), bottom = min(1, centre + 0.9 * line)
    let stripHeight = even(Double(width) * (bottom - top) * Double(videoHeight) / Double(videoWidth))
    // Where the rows sit in that strip, as Vision measures (from the bottom).
    let rowHeight = line / (bottom - top), lower = (bottom - centre) / (bottom - top), upper = lower + rowHeight * 1.15

    var shown: [String] = [], previous: [UInt8] = [], last = "", lastReport = -60.0
    try frames(media, tools, stream: stream, start: 0, length: nil, top: top, bottom: bottom, rate: sampleRate, width: width, height: stripHeight) { index, data in
        let picture = coarse(data, width, stripHeight)
        if !same(picture, previous) {
            let lines = read(data, width, stripHeight).filter { line in
                abs(line.midX - 0.5) < 0.08 && ideographs(line.text) >= 1
                    && line.height > rowHeight * 0.55 && line.height < rowHeight * 1.6
                    && (abs(line.midY - lower) < rowHeight * 0.45 || abs(line.midY - upper) < rowHeight * 0.45)
            }.sorted { $0.midY > $1.midY }
            last = lines.map { $0.text.trimmingCharacters(in: .whitespaces) }.joined(separator: " ")
            previous = picture
        }
        shown.append(last)
        let seconds = Double(index) / sampleRate
        if seconds - lastReport >= 30 {
            lastReport = seconds
            let body: [String: Any] = ["stage": "transcription", "completed_cues": Int(min(seconds, duration)), "total_cues": Int(duration.rounded(.up))]
            if let json = try? JSONSerialization.data(withJSONObject: body) { try? json.write(to: progress, options: .atomic) }
        }
        return true
    }
    guard Double(shown.count) / sampleRate > duration * 0.9 else { throw Failure(message: "Video decoding stopped early", status: 1) }
    let result = cues(from: shown, rate: sampleRate)
    // Dialogue runs to a dozen lines a minute; a sign or a caption now and then is not subtitles.
    guard Double(result.count) >= max(10, duration / 60 * 2) else { throw Failure(message: "No subtitles in the picture", status: 4) }
    let document = Output(language: "zh-Hans", method: "burned", characters: result.reduce(0) { $0 + ideographs($1.text) },
                          cue_seconds: result.reduce(0.0) { $0 + Double($1.end_ms - $1.start_ms) / 1000 }, duration_seconds: duration, cues: result)
    let encoded = try JSONEncoder().encode(document)
    guard encoded.count <= 16_777_216 else { throw Failure(message: "Subtitles too large", status: 1) }
    try encoded.write(to: output, options: .atomic)
}

@main struct BurnedSubtitles {
    static func main() {
        let args = CommandLine.arguments
        guard args.count == 4 || args.count == 5 else {
            FileHandle.standardError.write(Data("Expected media, result path, progress path and optionally the tools folder\n".utf8)); exit(2)
        }
        let tools = args.count == 5 ? URL(fileURLWithPath: args[4], isDirectory: true)
            : URL(fileURLWithPath: CommandLine.arguments[0]).resolvingSymlinksInPath().deletingLastPathComponent()
        do {
            try extract(media: args[1], output: URL(fileURLWithPath: args[2]), progress: URL(fileURLWithPath: args[3]), tools: tools)
        } catch let failure as Failure {
            FileHandle.standardError.write(Data("\(failure.message)\n".utf8)); exit(failure.status)
        } catch {
            // Never log subtitle text or media paths.
            FileHandle.standardError.write(Data("Reading subtitles from the picture failed: \((error as NSError).domain) \((error as NSError).code)\n".utf8)); exit(1)
        }
    }
}
