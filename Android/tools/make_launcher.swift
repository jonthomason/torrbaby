import Foundation
import CoreText
import CoreGraphics

// Vector wordmark: black fills the entire 16:9 TV banner, no inset icon tile.
let font = CTFontCreateWithName("Georgia-Bold" as CFString, 120, nil)
let text = NSAttributedString(string: "tb", attributes: [NSAttributedString.Key(kCTFontAttributeName as String): font])
let line = CTLineCreateWithAttributedString(text)
let combined = CGMutablePath()
for run in CTLineGetGlyphRuns(line) as! [CTRun] {
    let count = CTRunGetGlyphCount(run)
    var glyphs = [CGGlyph](repeating: 0, count: count)
    var positions = [CGPoint](repeating: .zero, count: count)
    CTRunGetGlyphs(run, CFRange(location: 0, length: 0), &glyphs)
    CTRunGetPositions(run, CFRange(location: 0, length: 0), &positions)
    for i in 0..<count {
        if let path = CTFontCreatePathForGlyph(font, glyphs[i], nil) {
            combined.addPath(path, transform: CGAffineTransform(translationX: positions[i].x, y: positions[i].y))
        }
    }
}
let bounds = combined.boundingBoxOfPath
let scale = 132 / bounds.height
var transform = CGAffineTransform(a: scale, b: 0, c: 0, d: -scale,
    tx: 160 - bounds.midX * scale, ty: 90 + bounds.midY * scale)
let path = combined.copy(using: &transform)!
func point(_ p: CGPoint) -> String { String(format: "%.3f,%.3f", p.x, p.y) }
var data = ""
path.applyWithBlock { pointer in
    let e = pointer.pointee
    switch e.type {
    case .moveToPoint: data += "M" + point(e.points[0])
    case .addLineToPoint: data += "L" + point(e.points[0])
    case .addQuadCurveToPoint: data += "Q" + point(e.points[0]) + " " + point(e.points[1])
    case .addCurveToPoint: data += "C" + point(e.points[0]) + " " + point(e.points[1]) + " " + point(e.points[2])
    case .closeSubpath: data += "Z"
    @unknown default: break
    }
}
print("""
<vector xmlns:android="http://schemas.android.com/apk/res/android" android:width="320dp" android:height="180dp" android:viewportWidth="320" android:viewportHeight="180">
    <path android:fillColor="#000000" android:pathData="M0,0H320V180H0Z"/>
    <path android:fillColor="#CB53F5" android:pathData="\(data)"/>
</vector>
""")
