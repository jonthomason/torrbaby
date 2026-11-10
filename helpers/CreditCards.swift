import Foundation
import CoreGraphics
import Vision

// Static credit-card detection ("cards-v1"). End credits are often a montage of
// still cards ("Directed by / NAME", cast and crew lists) on a plain background
// rather than a crawl. A single frame can resemble a sign or a caption, so a
// segment needs a sustained run of several distinct credit cards; ordinary
// scenes interrupt a run, while plain frames (fades, logos) do not.

struct CardLine { let text: String; let box: CGRect }
struct CardFrame { let time: Double; let lines: [CardLine]; let dark: Double }
enum CardLabel { case strong, weak, plain, blank, scene }

struct CardRun {
    let start: Double, end: Double
    let strong: Int, distinct: Int, plainShare: Double
    let signature: [String]
    var document: [String: Any] {
        ["start": start, "end": end, "method": "cards-v1", "confidence": (confidence * 100).rounded() / 100,
         "strong_cards": strong, "distinct_cards": distinct, "plain_share": (plainShare * 100).rounded() / 100, "signature": signature]
    }
    var confidence: Double {
        // Three distinct strong cards is the minimum; more distinct cards and a
        // plain background (credits, not on-screen text in a scene) add weight.
        min(0.95, 0.45 + 0.08 * Double(min(distinct, 8) - 3) + (plainShare >= 0.6 ? 0.1 : 0) + (distinct >= 6 ? 0.05 : 0))
    }
}

enum CreditCards {
    static let roleWords: [String] = [
        "produc", "execut", "direct", "written", "writer", "teleplay", "story by", "created by", "developed by",
        "based on", "edited", "editor", "editing", "music", "score", "compos", "casting", "costume", "design",
        "photograph", "cinematograph", "camera", "supervis", "coordinat", "manager", "assistant", "sound",
        "mixer", "visual effects", "special effects", "stunt", "make-up", "makeup", "hair", "decorat",
        "property master", "props", "grip", "gaffer", "lighting", "electric", "script", "consultant",
        "associate", "starring", "special thanks", "in association with", "location", "colorist",
        "colourist", "post production", "dialect", "choreograph", "transportation", "accountant",
        "title design", "animation", "recording",
        "导演", "编剧", "制片", "出品", "主演", "监制", "摄影", "剪辑", "美术", "音乐", "作曲", "录音", "混音",
        "灯光", "服装", "化妆", "造型", "道具", "制作", "统筹", "特效", "演员", "鸣谢", "联合", "总监", "配乐",
        "策划", "发行", "领衔", "出演",
    ]
    static let particles: Set<String> = ["de", "da", "van", "von", "la", "le", "del", "di", "du", "of", "and", "&"]

    static func isRole(_ text: String) -> Bool {
        let lower = text.lowercased()
        return roleWords.contains { lower.contains($0) }
    }

    // Personal or company names as credits print them: a few capitalised words
    // ("Michael Wilson", "MICHAEL WILSON", "J. David Brightbill") or 2-4 Han characters.
    static func isName(_ text: String) -> Bool {
        let s = text.trimmingCharacters(in: .whitespaces)
        let scalars = s.unicodeScalars
        if (2...4).contains(scalars.count) && scalars.allSatisfy({ (0x4E00...0x9FFF).contains($0.value) || $0 == "·" }) { return true }
        guard (4...40).contains(s.count), !s.contains(where: \.isNumber) else { return false }
        let tokens = s.replacingOccurrences(of: ",", with: " ").split(separator: " ").map(String.init)
        guard (1...5).contains(tokens.count) else { return false }
        for token in tokens where !particles.contains(token.lowercased()) {
            guard let first = token.first, first.isUppercase,
                  token.allSatisfy({ $0.isLetter || "'’.-".contains($0) }) else { return false }
        }
        return s.filter(\.isLetter).count >= 4 && (tokens.count >= 2 || s == s.uppercased())
    }

    static func label(_ frame: CardFrame) -> CardLabel {
        let lines = frame.lines.filter { $0.text.trimmingCharacters(in: .whitespaces).count >= 2 }
        // Footage without any text (between cards over a picture) neither extends nor breaks a run quickly.
        if lines.isEmpty { return frame.dark >= 0.8 ? .plain : .blank }
        let roles = lines.filter { isRole($0.text) }.count
        let names = lines.filter { !isRole($0.text) && isName($0.text) }.count
        let credit = Double(roles + names) / Double(lines.count)
        if roles >= 1 && names >= 1 && credit >= 0.6 { return .strong }
        if names >= 1 && credit >= 0.6 && frame.dark >= 0.85 { return .weak }
        if frame.dark >= 0.8 && lines.count <= 3 { return .plain }
        return .scene
    }

    static func signature(_ frame: CardFrame) -> Set<String> {
        Set(frame.lines.map { $0.text.lowercased().filter { $0.isLetter || $0.isNumber } }.filter { $0.count >= 4 })
    }

    static func similar(_ a: Set<String>, _ b: Set<String>) -> Bool {
        guard !a.isEmpty, !b.isEmpty else { return false }
        return Double(a.intersection(b).count) / Double(a.union(b).count) >= 0.5
    }

    /// Runs over frames sampled in time order. `sceneGap` is how much ordinary
    /// footage with other text may interrupt a run; `blankGap` bounds textless
    /// footage and `plainGap` near-black fades and logos.
    static func runs(_ frames: [CardFrame], sceneGap: Double = 8, blankGap: Double = 20, plainGap: Double = 30) -> [CardRun] {
        var result = [CardRun](), current = [(CardFrame, CardLabel)](), scene = 0.0
        var previous: Double?
        func close() {
            defer { current = []; scene = 0 }
            let cards = current.filter { $0.1 == .strong || $0.1 == .weak }
            let strong = cards.filter { $0.1 == .strong }
            var distinct = [Set<String>]()
            for (frame, _) in strong {
                let s = signature(frame)
                if !distinct.contains(where: { similar($0, s) }) { distinct.append(s) }
            }
            guard let first = cards.first?.0, let last = cards.last?.0,
                  strong.count >= 3, distinct.count >= 3, last.time - first.time >= 8 else { return }
            let plain = Double(cards.filter { $0.0.dark >= 0.85 }.count) / Double(cards.count)
            var seen = Set<String>(), words = [String]()
            for (frame, _) in cards {
                for line in frame.lines where isName(line.text) && !isRole(line.text) {
                    let key = line.text.lowercased().filter { $0.isLetter || $0.isNumber }
                    if key.count >= 4 && words.count < 120 && seen.insert(key).inserted { words.append(key) }
                }
            }
            result.append(CardRun(start: first.time, end: last.time, strong: strong.count,
                                  distinct: distinct.count, plainShare: plain, signature: words))
        }
        for frame in frames.sorted(by: { $0.time < $1.time }) {
            let dt = previous.map { frame.time - $0 } ?? 0
            previous = frame.time
            let kind = label(frame)
            switch kind {
            case .strong, .weak:
                current.append((frame, kind)); scene = 0
            case .plain, .blank, .scene:
                guard let index = current.lastIndex(where: { $0.1 == .strong || $0.1 == .weak }) else { continue }
                let last = current[index]
                if kind == .scene { scene += dt }
                let footage = kind == .blank || current[(index + 1)...].contains { $0.1 == .blank }
                let limit = footage ? blankGap : plainGap
                if scene > sceneGap || frame.time - last.0.time > limit { close() }
                else { current.append((frame, kind)) }
            }
        }
        close()
        return result
    }

    static func frame(time: Double, image: CGImage, observations: [VNRecognizedTextObservation]) -> CardFrame {
        let lines = observations.compactMap { observation -> CardLine? in
            guard let text = observation.topCandidates(1).first, text.confidence >= 0.4 else { return nil }
            return CardLine(text: text.string, box: observation.boundingBox)
        }
        return CardFrame(time: time, lines: lines, dark: darkness(image))
    }

    /// Coarse sweep of the analysis window (reusing frames already sampled), then
    /// 2-second refinement around anything card-like. `coarseStep` must be fine
    /// enough to land on a short TV card sequence; film windows are longer, with
    /// longer credits, and use a wider step to stay within the frame budget.
    /// `sample` returns nil for an unreadable position, which is skipped.
    static func scan(lower: Double, duration: Double, coarseStep: Double = 6, frames initial: [CardFrame],
                     extraLimit: Int = 160, sample: (Double) throws -> CardFrame?) throws -> [CardRun] {
        var frames = initial
        var seeds = initial.filter { [.strong, .weak].contains(label($0)) }
        func at(_ time: Double, within tolerance: Double) throws -> Bool {
            if frames.contains(where: { abs($0.time - time) < tolerance }) { return false }
            guard let frame = try sample(time) else { return false }
            frames.append(frame)
            if [.strong, .weak].contains(label(frame)) { seeds.append(frame) }
            return true
        }
        var time = duration - 2
        while time >= lower { _ = try at(time, within: 1.5); time -= coarseStep }
        // Any frame that looks like a card gets its coarse neighbourhood filled in at
        // 2 s, transitively (latest first), so short or interrupted card sequences are
        // seen whole. A stray sign costs a few extra frames; the run rules still apply.
        var extra = 0, searched = Set<Double>()
        while extra < extraLimit {
            while extra < extraLimit, let seed = seeds.max(by: { $0.time < $1.time }) {
                seeds.removeAll { $0.time == seed.time }
                for offset in stride(from: -coarseStep - 2, through: coarseStep + 2, by: 2) where offset != 0 {
                    let t = seed.time + offset
                    guard t >= lower, t < duration, extra < extraLimit else { continue }
                    if try at(t, within: 0.9) { extra += 1 }
                }
            }
            // Earlier cards can sit behind a longer textless stretch (a title card,
            // credits over a quiet shot) than the neighbourhood covers: look back
            // across the whole tolerated gap before each run's first card.
            for run in runs(frames) where searched.insert(run.start).inserted {
                for t in stride(from: run.start - 2, through: max(lower, run.start - 20), by: -2) where extra < extraLimit {
                    if try at(t, within: 0.9) { extra += 1 }
                }
            }
            if seeds.isEmpty { break }
        }
        return runs(frames)
    }

    /// Fraction of the frame that is near-black, from a 32x18 grey thumbnail.
    static func darkness(_ image: CGImage) -> Double {
        let w = 32, h = 18
        var pixels = [UInt8](repeating: 0, count: w * h)
        guard let context = CGContext(data: &pixels, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w,
                                      space: CGColorSpaceCreateDeviceGray(), bitmapInfo: CGImageAlphaInfo.none.rawValue) else { return 0 }
        context.interpolationQuality = .medium
        context.draw(image, in: CGRect(x: 0, y: 0, width: w, height: h))
        return Double(pixels.filter { $0 < 30 }.count) / Double(pixels.count)
    }

    static func selfTest() -> Bool {
        func card(_ t: Double, _ texts: [String], dark: Double = 0.95) -> CardFrame {
            CardFrame(time: t, lines: texts.enumerated().map { CardLine(text: $1, box: CGRect(x: 0.35, y: 0.55 - Double($0) * 0.05, width: 0.3, height: 0.03)) }, dark: dark)
        }
        let scene = (0..<20).map { card(Double($0) * 2, ["EXIT", "SOUTH GEORGIA"], dark: 0.2) }
        let cards = [card(40, ["Directed by", "ALEX ORR"]), card(42, ["Music by", "NATALIE HOLT"]),
                     card(44, []), card(46, ["Casting by", "SUSIE FARRIS"]), card(48, ["PERRY MATTFELD"]),
                     card(50, ["Edited by", "LISA LASSEK"]), card(52, ["Production Designer", "CAREY MEYER"])]
        let found = runs(scene + cards)
        // Repeating one card (a sign held on screen) is not a sequence of credits.
        let held = runs((0..<10).map { card(Double($0) * 2, ["Directed by", "ALEX ORR"]) })
        // Credit words inside an ordinary bright scene with other text are not cards.
        let signs = runs((0..<10).map { card(Double($0) * 2, ["Production Office \($0)", "KEEP OUT", "Floor 3 Room 12", "parking"], dark: 0.1) })
        // Cards over a picture with textless footage between them are one run, and a
        // Chinese card sequence reads the same way.
        let quiet = (0..<6).map { card(Double($0) * 2 + 2, [], dark: 0.3) }
        let over = runs([card(0, ["Executive Producer", "LARRY DAVID"], dark: 0.7)] + quiet +
                        [card(14, ["Producer", "JERRY SEINFELD"], dark: 0.7), card(16, ["Story Editor", "MATT GOLDMAN"], dark: 0.7)])
        let chinese = runs([card(0, ["导演", "王家卫"]), card(3, ["编剧", "刘以鬯"]), card(6, ["摄影", "杜可风"]), card(9, ["主演", "梁朝伟", "张曼玉"])])
        return found.count == 1 && found[0].start == 40 && found[0].end == 52 && found[0].distinct == 5 &&
            held.isEmpty && signs.isEmpty && isName("J. David Brightbill") && isName("王家卫") &&
            !isName("Floor 3") && isRole("导演") && over.count == 1 && over[0].start == 0 && over[0].plainShare == 0 &&
            chinese.count == 1 && chinese[0].distinct == 4
    }
}
