import Foundation
import AVFoundation
import Vision
import CoreGraphics

struct Line { let text: String; let box: CGRect }
struct Sample { let time: Double; let lines: [Line] }
struct ScanError: Error { let message: String }

// Match actual words as well as their geometry: camera motion or changing captions
// must not be mistaken for multiple persistent lines of rolling credit text.
func rolling(_ a: Sample, _ b: Sample) -> Bool {
    let dt = b.time - a.time
    guard dt > 0.3, dt < 3 else { return false }
    var matches: [(Double, Double)] = []
    for line in a.lines where line.text.count >= 4 {
        let candidates = b.lines.filter {
            $0.text == line.text && abs($0.box.midX-line.box.midX) < 0.035 &&
            abs($0.box.width-line.box.width) < 0.045
        }
        guard candidates.count == 1, let next = candidates.first else { continue }
        let speed = (next.box.midY-line.box.midY)/dt
        if speed > 0.004 && speed < 0.16 { matches.append((speed,line.box.midY)) }
    }
    guard matches.count >= 5 else { return false }
    let speeds = matches.map(\.0).sorted(), median = speeds[speeds.count/2]
    let aligned = matches.filter { abs($0.0-median) < max(0.003,median*0.20) }
    return aligned.count >= 5 && Double(aligned.count)/Double(matches.count) >= 0.8 &&
        (aligned.map(\.1).max()! - aligned.map(\.1).min()!) > 0.22
}

@main struct CreditsScanner {
    static func main() async {
        do { try await run() }
        catch {
            let message = (error as? ScanError)?.message ?? error.localizedDescription
            FileHandle.standardError.write(Data(message.utf8)); exit(1)
        }
    }
    static func run() async throws {
        let args = CommandLine.arguments
        if args.count == 2 && args[1] == "--self-test" {
            let lines = (0..<8).map { Line(text:"person name \($0)",box:CGRect(x:0.25,y:0.1+Double($0)*0.08,width:0.5,height:0.025)) }
            let moved = lines.map { Line(text:$0.text,box:$0.box.offsetBy(dx:0,dy:0.025)) }
            guard rolling(Sample(time:0,lines:lines),Sample(time:1,lines:moved)),
                  !rolling(Sample(time:0,lines:lines),Sample(time:1,lines:lines)),
                  !rolling(Sample(time:0,lines:Array(lines.prefix(2))),Sample(time:1,lines:Array(moved.prefix(2)))) else {
                throw ScanError(message:"Motion checks failed")
            }
            print("rolling, static, subtitle-line checks passed"); return
        }
        guard args.count == 4, let budget = Double(args[2]), [300.0,900.0].contains(budget) else {
            throw ScanError(message:"Expected input, 300/900-second window, progress file")
        }
        let input = args[1].hasPrefix("http://127.0.0.1:") ? URL(string:args[1])! : URL(fileURLWithPath:args[1])
        let asset = AVURLAsset(url:input)
        let duration = try await asset.load(.duration).seconds
        guard duration.isFinite, duration > 5 else { throw ScanError(message:"Invalid duration") }
        let generator = AVAssetImageGenerator(asset:asset)
        generator.appliesPreferredTrackTransform = true
        generator.maximumSize = CGSize(width:1280,height:720)
        generator.requestedTimeToleranceBefore = CMTime(seconds:0.1,preferredTimescale:600)
        generator.requestedTimeToleranceAfter = CMTime(seconds:0.1,preferredTimescale:600)
        let started = Date(), lower = max(0,duration-budget), step = 20.0
        var frames = 0, probes = 0, hits = [Double](), errors = 0
        func sample(_ time:Double) throws -> Sample {
            try autoreleasepool {
                var actual = CMTime.zero
                let image = try generator.copyCGImage(at:CMTime(seconds:time,preferredTimescale:600),actualTime:&actual)
                let request = VNRecognizeTextRequest()
                request.recognitionLevel = .accurate
                request.recognitionLanguages = ["en-US","zh-Hans"]
                request.usesLanguageCorrection = false
                request.minimumTextHeight = 0.012
                try VNImageRequestHandler(cgImage:image,options:[:]).perform([request])
                frames += 1
                let lines = (request.results ?? []).compactMap { observation -> Line? in
                    guard let text = observation.topCandidates(1).first, text.confidence >= 0.4 else { return nil }
                    let normalized = text.string.lowercased().filter { $0.isLetter || $0.isNumber }
                    return Line(text:normalized,box:observation.boundingBox)
                }
                return Sample(time:actual.seconds,lines:lines)
            }
        }
        func probe(_ time:Double) throws -> Bool {
            guard Date().timeIntervalSince(started) < 240 else { throw ScanError(message:"Credits scan time budget reached") }
            probes += 1
            let a = try sample(time)
            guard a.lines.count >= 5 else { return false }
            let b = try sample(time+1)
            guard rolling(a,b) else { return false }
            return rolling(b,try sample(time+2))
        }
        var time = max(lower,duration-4)
        while time >= lower {
            do { if try probe(time) { hits.append(time) } } catch { errors += 1; if errors > 4 { throw error } }
            let data = try JSONSerialization.data(withJSONObject:["scanned_seconds":duration-time,"window_seconds":min(budget,duration),"frames":frames])
            try data.write(to:URL(fileURLWithPath:args[3]),options:.atomic)
            if time == lower { break }; time = max(lower,time-step)
        }
        // Require at least two positive probes 20 seconds apart. Static cards and
        // one brief camera movement cannot create a credit boundary.
        let ascending = hits.sorted()
        var groups = [[Double]]()
        for t in ascending {
            if let last = groups.last?.last, t-last <= step+0.1 { groups[groups.count-1].append(t) }
            else { groups.append([t]) }
        }
        var segments = [[String:Double]]()
        for group in groups where group.count >= 2 {
            var start = group.first!, end = group.last!+2
            var t = max(lower,start-step)
            while t < start {
                if (try? probe(t)) == true { start=t; break }; t += 2
            }
            t = end
            while t+2 < min(duration,group.last!+step) {
                guard (try? probe(t)) == true else { break }; end=t+2; t += 2
            }
            // A rolling sequence already in progress at the scan limit has no
            // observed start. Do not present the cap as a detected boundary.
            if start > lower+2 { segments.append(["start":start,"end":end]) }
        }
        let result:[String:Any] = ["state":errors == 0 ? "ready" : "incomplete","duration":duration,"segments":segments,
            "method":"rolling-text-v1","frames":frames,"probes":probes,"sample_errors":errors,
            "elapsed_seconds":Date().timeIntervalSince(started),"window_seconds":min(budget,duration)]
        print(String(data:try JSONSerialization.data(withJSONObject:result,options:[.sortedKeys]),encoding:.utf8)!)
    }
}
