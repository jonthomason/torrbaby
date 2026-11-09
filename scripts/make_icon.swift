import AppKit
import CoreText

// Deterministic typography: lowercase letters, no generated or bundled font assets.
let destination = CommandLine.arguments[1]
let font = CTFontCreateWithName("Georgia-Bold" as CFString, 800, nil)
let text = NSAttributedString(string: "tb", attributes: [.font: font])
let line = CTLineCreateWithAttributedString(text)
let letters = CGMutablePath()
for run in CTLineGetGlyphRuns(line) as! [CTRun] {
    let count = CTRunGetGlyphCount(run)
    var glyphs = [CGGlyph](repeating: 0, count: count)
    var positions = [CGPoint](repeating: .zero, count: count)
    CTRunGetGlyphs(run, CFRange(location: 0, length: 0), &glyphs)
    CTRunGetPositions(run, CFRange(location: 0, length: 0), &positions)
    for index in 0..<count {
        if let path = CTFontCreatePathForGlyph(font, glyphs[index], nil) {
            letters.addPath(path, transform: CGAffineTransform(translationX: positions[index].x, y: positions[index].y))
        }
    }
}
let bounds = letters.boundingBoxOfPath
let scale = min(760 / bounds.width, 680 / bounds.height)
let transform = CGAffineTransform(a: scale, b: 0, c: 0, d: scale,
    tx: (1024 - bounds.width * scale) / 2 - bounds.minX * scale,
    ty: (1024 - bounds.height * scale) / 2 - bounds.minY * scale)
for size in [16, 32, 64, 128, 256, 512, 1024] {
    let context = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8,
        bytesPerRow: size * 4, space: CGColorSpace(name: CGColorSpace.sRGB)!,
        bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)!
    context.scaleBy(x: CGFloat(size) / 1024, y: CGFloat(size) / 1024)
    context.setFillColor(CGColor(red: 0, green: 0, blue: 0, alpha: 1))
    context.fill(CGRect(x: 0, y: 0, width: 1024, height: 1024))
    context.concatenate(transform)
    context.setFillColor(CGColor(red: 0.73, green: 0.24, blue: 1, alpha: 1))
    context.addPath(letters)
    context.fillPath()
    let image = NSBitmapImageRep(cgImage: context.makeImage()!)
    try image.representation(using: .png, properties: [:])!.write(
        to: URL(fileURLWithPath: destination + "/icon-\(size).png"))
}
