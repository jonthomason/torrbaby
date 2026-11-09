import AppKit
let dest = CommandLine.arguments[1]
let sizes = [16, 32, 64, 128, 256, 512, 1024]
for size in sizes {
    let c = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: size * 4, space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)!
    c.scaleBy(x: CGFloat(size) / 1024, y: CGFloat(size) / 1024)
    let rect = CGRect(x: 0, y: 0, width: 1024, height: 1024)
    c.setFillColor(NSColor(calibratedRed: 0.055, green: 0.09, blue: 0.13, alpha: 1).cgColor)
    c.fill(rect)
    let gradient = CGGradient(colorsSpace: CGColorSpaceCreateDeviceRGB(), colors: [NSColor(calibratedRed: 0.17, green: 0.32, blue: 0.32, alpha: 1).cgColor, NSColor(calibratedRed: 0.055, green: 0.09, blue: 0.13, alpha: 1).cgColor] as CFArray, locations: [0, 1])!
    c.drawLinearGradient(gradient, start: CGPoint(x: 150, y: 1024), end: CGPoint(x: 900, y: 0), options: [])
    let mint = NSColor(calibratedRed: 0.44, green: 0.91, blue: 0.76, alpha: 1).cgColor
    c.setStrokeColor(mint)
    c.setLineWidth(54)
    c.setLineCap(.round)
    c.setLineJoin(.round)
    c.move(to: CGPoint(x: 512, y: 760)); c.addLine(to: CGPoint(x: 512, y: 370))
    c.move(to: CGPoint(x: 350, y: 520)); c.addLine(to: CGPoint(x: 512, y: 358)); c.addLine(to: CGPoint(x: 674, y: 520)); c.strokePath()
    c.setLineWidth(46)
    c.move(to: CGPoint(x: 245, y: 330)); c.addLine(to: CGPoint(x: 245, y: 260)); c.addQuadCurve(to: CGPoint(x: 305, y: 200), control: CGPoint(x: 245, y: 200)); c.addLine(to: CGPoint(x: 719, y: 200)); c.addQuadCurve(to: CGPoint(x: 779, y: 260), control: CGPoint(x: 779, y: 200)); c.addLine(to: CGPoint(x: 779, y: 330)); c.strokePath()
    let rep = NSBitmapImageRep(cgImage: c.makeImage()!)
    try rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: dest + "/icon-\(size).png"))
}
