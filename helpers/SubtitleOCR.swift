import Foundation
import Vision
import NaturalLanguage
import CoreGraphics
import CoreText
import ImageIO
import UniformTypeIdentifiers

struct Cue: Codable { let file:String;let width:Int;let height:Int;let start_ms:Int;let end_ms:Int?;let forced:Bool }
struct Manifest: Decodable { let cues:[Cue];let extraction_seconds:Double }
func image(_ cue:Cue,_ root:URL) throws -> CGImage {
 let data=try Data(contentsOf:root.appendingPathComponent(cue.file))
 guard data.count==cue.width*cue.height, let provider=CGDataProvider(data:data as CFData),let source=CGImage(width:cue.width,height:cue.height,bitsPerComponent:8,bitsPerPixel:8,bytesPerRow:cue.width,space:CGColorSpaceCreateDeviceGray(),bitmapInfo:CGBitmapInfo(rawValue:0),provider:provider,decode:nil,shouldInterpolate:false,intent:.defaultIntent) else {throw NSError(domain:"Image",code:1)}
 let pad=24,scale=2,w=cue.width*scale+pad*2,h=cue.height*scale+pad*2
 guard let context=CGContext(data:nil,width:w,height:h,bitsPerComponent:8,bytesPerRow:w,space:CGColorSpaceCreateDeviceGray(),bitmapInfo:0) else {throw NSError(domain:"Image",code:2)}
 context.setFillColor(gray:1,alpha:1);context.fill(CGRect(x:0,y:0,width:w,height:h));context.interpolationQuality = .none
 context.draw(source,in:CGRect(x:pad,y:pad,width:cue.width*scale,height:cue.height*scale))
 return context.makeImage()!
}
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
// Keep this block identical in CreditsScanner.swift.
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
struct Output: Codable { let language:String;let method:String;let unreadableCues:Int;let cues:[TextCue] }
struct TextCue: Codable {let start_ms:Int;let end_ms:Int;let text:String}
@main struct OCR {
 static func main() throws {
  let args=CommandLine.arguments
  guard args.count == 5 else { throw NSError(domain:"Expected image directory, result path, language hint, progress path",code:1) }
  let root=URL(fileURLWithPath:args[1]), output=URL(fileURLWithPath:args[2]), hint=args[3], progress=URL(fileURLWithPath:args[4])
  let data=try Data(contentsOf:root.appendingPathComponent("manifest.json"))
  guard data.count <= 4_194_304 else {throw NSError(domain:"Large OCR manifest",code:1)}
  let manifest=try JSONDecoder().decode(Manifest.self,from:data)
  guard !manifest.cues.isEmpty, manifest.cues.count <= 10000, ["auto","en","zh-Hans"].contains(hint) else {throw NSError(domain:"Invalid OCR request",code:1)}
  do {try prepareTextRecognition()} catch {modelsChanged(" (warm-up: \(error))")}
  var cues=[TextCue](),unreadable=0
  for (index,cue) in manifest.cues.enumerated() {
   guard cue.file == String(format:"%05d.gray",index), cue.width>0,cue.height>0,cue.width<=4096,cue.height<=2160,
         let end=cue.end_ms,end>cue.start_ms else {throw NSError(domain:"Invalid OCR cue",code:1)}
   let text=try autoreleasepool { () throws -> String in
    let bitmap=try image(cue,root)
    let request=VNRecognizeTextRequest();request.recognitionLevel = .accurate
    request.recognitionLanguages=hint == "en" ? ["en-US"] : hint == "zh-Hans" ? ["zh-Hans"] : ["en-US","zh-Hans"]
    request.usesLanguageCorrection=true
    recognize(request,bitmap)
    if (request.results ?? []).isEmpty {
     let w=max(800,bitmap.width+100),h=max(300,bitmap.height+100)
     let c=CGContext(data:nil,width:w,height:h,bitsPerComponent:8,bytesPerRow:w,space:CGColorSpaceCreateDeviceGray(),bitmapInfo:0)!
     c.setFillColor(gray:1,alpha:1);c.fill(CGRect(x:0,y:0,width:w,height:h));c.draw(bitmap,in:CGRect(x:(w-bitmap.width)/2,y:(h-bitmap.height)/2,width:bitmap.width,height:bitmap.height))
     let padded=c.makeImage()!
     recognize(request,padded)
     if (request.results ?? []).isEmpty {
      request.recognitionLevel = .fast
      // Vision's fast recognizer does not support every accurate-path language.
      request.recognitionLanguages=["en-US"]
      if hint != "zh-Hans" {recognize(request,padded)}
     }
    }
    let observations=(request.results ?? []).sorted {a,b in abs(a.boundingBox.midY-b.boundingBox.midY)>0.03 ? a.boundingBox.midY>b.boundingBox.midY:a.boundingBox.minX<b.boundingBox.minX}
    return observations.compactMap {$0.topCandidates(1).first?.string}.joined(separator:"\n").trimmingCharacters(in:.whitespacesAndNewlines)
   }
   if text.isEmpty {unreadable += 1} else {
    guard text.utf8.count<=16384 else {throw NSError(domain:"Large OCR result",code:1)}
    cues.append(TextCue(start_ms:cue.start_ms,end_ms:end,text:text))
   }
   if index%16 == 0 || index+1 == manifest.cues.count {
    let d=try JSONSerialization.data(withJSONObject:["completed_cues":index+1,"total_cues":manifest.cues.count,"stage":"ocr"])
    try d.write(to:progress,options:.atomic)
   }
  }
  guard !cues.isEmpty,unreadable <= max(3,manifest.cues.count/100) else {throw NSError(domain:"Too many unreadable subtitle images",code:1)}
  let recognizer=NLLanguageRecognizer();recognizer.processString(String(cues.map(\.text).joined(separator:" ").prefix(30000)))
  let tag=recognizer.dominantLanguage?.rawValue ?? ""
  let language=tag == "en" ? "en" : tag.hasPrefix("zh") ? "zh-Hans" : ""
  guard !language.isEmpty, hint == "auto" || hint == language else {throw NSError(domain:"Subtitle language could not be verified",code:1)}
  let result=try JSONEncoder().encode(Output(language:language,method:"ocr",unreadableCues:unreadable,cues:cues))
  guard result.count<=16_777_216 else {throw NSError(domain:"Large OCR output",code:1)}
  try result.write(to:output,options:.atomic)
 }
}
