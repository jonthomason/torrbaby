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

let info: [String: Any] = ["author": "xcode", "version": 1]
func json(_ path: String, _ object: [String: Any]) throws {
    try FileManager.default.createDirectory(atPath: path, withIntermediateDirectories: true)
    try JSONSerialization.data(withJSONObject: object, options: [.prettyPrinted, .sortedKeys]).write(to: URL(fileURLWithPath: path + "/Contents.json"))
}
var assets = [[String: Any]]()
for (name, width, height, role) in [("Small",400,240,"primary-app-icon"),("Large",1280,768,"primary-app-icon"),("Shelf",1920,720,"top-shelf-image"),("WideShelf",2320,720,"top-shelf-image-wide")] {
    let icon = role == "primary-app-icon"
    let filename = name + (icon ? ".imagestack" : ".imageset")
    let root = destination + "/" + filename
    assets.append(["filename": filename, "idiom": "tv", "role": role, "size": "\(width)x\(height)"])
    let layers = icon ? ["Foreground", "Background"] : ["Composite"]
    if icon { try json(root, ["info": info, "layers": layers.map { ["filename": $0 + ".imagestacklayer"] }]) }
    for layer in layers {
        let folder = icon ? root + "/" + layer + ".imagestacklayer/Content.imageset" : root
        var images = [[String: Any]]()
        for density in (name == "Large" ? [1] : [1,2]) {
            let w = width * density, h = height * density
            let context = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4, space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
            if layer != "Foreground" { context.setFillColor(CGColor(gray: 0, alpha: 1)); context.fill(CGRect(x: 0, y: 0, width: w, height: h)) }
            if layer != "Background" {
                let bounds = letters.boundingBoxOfPath
                let scale = min(CGFloat(w) * 0.68 / bounds.width, CGFloat(h) * 0.68 / bounds.height)
                context.concatenate(CGAffineTransform(a: scale, b: 0, c: 0, d: scale, tx: (CGFloat(w)-bounds.width*scale)/2-bounds.minX*scale, ty: (CGFloat(h)-bounds.height*scale)/2-bounds.minY*scale))
                context.setFillColor(CGColor(red: 0.73, green: 0.24, blue: 1, alpha: 1)); context.addPath(letters); context.fillPath()
            }
            try FileManager.default.createDirectory(atPath: folder, withIntermediateDirectories: true)
            let image = NSBitmapImageRep(cgImage: context.makeImage()!)
            let png = "image-\(density)x.png"
            try image.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: folder + "/" + png))
            images.append(["filename": png, "idiom": "tv", "scale": "\(density)x"])
        }
        try json(folder, ["info": info, "images": images])
    }
}
try json(destination, ["info": info, "assets": assets])
