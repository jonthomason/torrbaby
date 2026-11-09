import Foundation
import Vision
import CoreGraphics
import ImageIO
@main struct Retry {
 static func main() throws {
  let root=URL(fileURLWithPath:CommandLine.arguments[1])
  for name in ["00032.png","00688.png"] {
   let source=CGImageSourceCreateWithURL(root.appendingPathComponent(name) as CFURL,nil)!,original=CGImageSourceCreateImageAtIndex(source,0,nil)!
   for padded in [false,true] {
    var bitmap=original
    if padded {let w=max(800,original.width+100),h=max(300,original.height+100);let c=CGContext(data:nil,width:w,height:h,bitsPerComponent:8,bytesPerRow:w,space:CGColorSpaceCreateDeviceGray(),bitmapInfo:0)!;c.setFillColor(gray:1,alpha:1);c.fill(CGRect(x:0,y:0,width:w,height:h));c.draw(original,in:CGRect(x:(w-original.width)/2,y:(h-original.height)/2,width:original.width,height:original.height));bitmap=c.makeImage()!}
    for level in [VNRequestTextRecognitionLevel.accurate,.fast] {
     let req=VNRecognizeTextRequest();req.recognitionLevel=level;req.recognitionLanguages=["en-US"];req.usesLanguageCorrection=true;req.minimumTextHeight=0
     try VNImageRequestHandler(cgImage:bitmap,options:[:]).perform([req]);print(name,"padded",padded,"accurate",level == .accurate,req.results?.compactMap{$0.topCandidates(1).first?.string} ?? [])
    }
   }
  }
 }
}
