import Foundation
import Vision
import CoreGraphics
import ImageIO
import UniformTypeIdentifiers

struct Cue: Codable { let file:String;let width:Int;let height:Int;let start_ms:Int;let end_ms:Int?;let forced:Bool }
struct Manifest: Decodable { let cues:[Cue];let extraction_seconds:Double }
struct ResultRow: Codable {let index:Int;let start_ms:Int;let end_ms:Int?;let text:String;let confidence:Float;let seconds:Double;let error:String?;let method:String}
func image(_ cue:Cue,_ root:URL) throws -> CGImage {
 let data=try Data(contentsOf:root.appendingPathComponent(cue.file))
 guard data.count==cue.width*cue.height, let provider=CGDataProvider(data:data as CFData),let source=CGImage(width:cue.width,height:cue.height,bitsPerComponent:8,bitsPerPixel:8,bytesPerRow:cue.width,space:CGColorSpaceCreateDeviceGray(),bitmapInfo:CGBitmapInfo(rawValue:0),provider:provider,decode:nil,shouldInterpolate:false,intent:.defaultIntent) else {throw NSError(domain:"Image",code:1)}
 let pad=24,scale=2,w=cue.width*scale+pad*2,h=cue.height*scale+pad*2
 guard let context=CGContext(data:nil,width:w,height:h,bitsPerComponent:8,bytesPerRow:w,space:CGColorSpaceCreateDeviceGray(),bitmapInfo:0) else {throw NSError(domain:"Image",code:2)}
 context.setFillColor(gray:1,alpha:1);context.fill(CGRect(x:0,y:0,width:w,height:h));context.interpolationQuality = .none
 context.draw(source,in:CGRect(x:pad,y:pad,width:cue.width*scale,height:cue.height*scale))
 return context.makeImage()!
}
@main struct OCR {
 static func main() throws {
 let args=CommandLine.arguments,root=URL(fileURLWithPath:args[1]),out=URL(fileURLWithPath:args[2]),workers=Int(args[3])!
 let manifest=try JSONDecoder().decode(Manifest.self,from:Data(contentsOf:root.appendingPathComponent("manifest.json")))
 try FileManager.default.createDirectory(at:out,withIntermediateDirectories:false)
 let limit=args.count>4 ? min(Int(args[4])!,manifest.cues.count):manifest.cues.count
 let lock=NSLock();var results=[ResultRow]();let started=Date();let queue=OperationQueue();queue.maxConcurrentOperationCount=workers
 for index in 0..<limit {
  queue.addOperation { autoreleasepool {
   let cue=manifest.cues[index],start=Date();var text="",confidence:Float=0,error:String?=nil,method="accurate"
   do {
    let bitmap=try image(cue,root)
    let request=VNRecognizeTextRequest();request.recognitionLevel = .accurate;request.recognitionLanguages=["en-US"];request.usesLanguageCorrection=true
    try VNImageRequestHandler(cgImage:bitmap,options:[:]).perform([request])
    if (request.results ?? []).isEmpty {
     let w=max(800,bitmap.width+100),h=max(300,bitmap.height+100)
     let c=CGContext(data:nil,width:w,height:h,bitsPerComponent:8,bytesPerRow:w,space:CGColorSpaceCreateDeviceGray(),bitmapInfo:0)!
     c.setFillColor(gray:1,alpha:1);c.fill(CGRect(x:0,y:0,width:w,height:h))
     c.draw(bitmap,in:CGRect(x:(w-bitmap.width)/2,y:(h-bitmap.height)/2,width:bitmap.width,height:bitmap.height))
     let padded=c.makeImage()!;method="accurate-padded"
     try VNImageRequestHandler(cgImage:padded,options:[:]).perform([request])
     if (request.results ?? []).isEmpty {
      request.recognitionLevel = .fast;method="fast-padded"
      try VNImageRequestHandler(cgImage:padded,options:[:]).perform([request])
     }
    }
    let observations=(request.results ?? []).sorted {a,b in abs(a.boundingBox.midY-b.boundingBox.midY)>0.03 ? a.boundingBox.midY>b.boundingBox.midY:a.boundingBox.minX<b.boundingBox.minX}
    let candidates=observations.compactMap {$0.topCandidates(1).first};text=candidates.map(\.string).joined(separator:"\n");confidence=candidates.map(\.confidence).min() ?? 0
    if index%25==0 || confidence<0.5 {
     if let dest=CGImageDestinationCreateWithURL(out.appendingPathComponent(String(format:"%05d.png",index)) as CFURL,UTType.png.identifier as CFString,1,nil) {CGImageDestinationAddImage(dest,bitmap,nil);CGImageDestinationFinalize(dest)}
    }
   } catch let e {error=e.localizedDescription}
   let row=ResultRow(index:index,start_ms:cue.start_ms,end_ms:cue.end_ms,text:text,confidence:confidence,seconds:Date().timeIntervalSince(start),error:error,method:method)
   lock.lock();results.append(row);lock.unlock()
  }}
 }
 queue.waitUntilAllOperationsAreFinished();let seconds=Date().timeIntervalSince(started);results.sort {$0.index<$1.index}
 let encoder=JSONEncoder();encoder.outputFormatting=[.prettyPrinted,.sortedKeys];try encoder.encode(results).write(to:out.appendingPathComponent("results.json"))
 let summary:[String:Any]=["retries":results.filter{$0.method != "accurate"}.count,"workers":workers,"cues":results.count,"wall_seconds":seconds,"cues_per_second":Double(results.count)/seconds,"empty":results.filter{$0.text.isEmpty}.count,"errors":results.filter{$0.error != nil}.count,"low_confidence":results.filter{$0.confidence<0.5}.count,"extraction_seconds":manifest.extraction_seconds]
 let data=try JSONSerialization.data(withJSONObject:summary,options:[.prettyPrinted,.sortedKeys]);try data.write(to:out.appendingPathComponent("summary.json"));print(String(data:data,encoding:.utf8)!)
 }
}
