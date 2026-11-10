import Foundation
import Vision
import NaturalLanguage
import CoreGraphics
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
// them through the system ANECompilerService. Compiled models are cached per
// executable name, but concurrent processes do not share a compile that is
// still running. Each one compiles the same models again, the system compiler
// saturates, and requests fail after about two minutes (CRImageReaderError,
// e5rt code 13) before anything is cached. So helpers compile under one
// cross-process lock, recognizing a blank image to load every recognizer model.
// The other helpers wait for the lock and then load the cached models in
// milliseconds. Keep this block identical in CreditsScanner.swift.
func prepareTextRecognition() throws {
 let caches=FileManager.default.urls(for:.cachesDirectory,in:.userDomainMask)[0].appendingPathComponent("org.torrbaby.server",isDirectory:true)
 try FileManager.default.createDirectory(at:caches,withIntermediateDirectories:true)
 let fd=open(caches.appendingPathComponent("text-recognition.lock").path,O_RDWR|O_CREAT|O_CLOEXEC,0o600)
 guard fd >= 0 else {throw NSError(domain:"Text recognition lock unavailable",code:1)}
 defer {close(fd)}
 guard flock(fd,LOCK_EX) == 0 else {throw NSError(domain:"Text recognition lock unavailable",code:1)}
 defer {flock(fd,LOCK_UN)}
 guard let c=CGContext(data:nil,width:64,height:64,bitsPerComponent:8,bytesPerRow:64,space:CGColorSpaceCreateDeviceGray(),bitmapInfo:0) else {throw NSError(domain:"Text recognition warm-up failed",code:1)}
 c.setFillColor(gray:1,alpha:1);c.fill(CGRect(x:0,y:0,width:64,height:64))
 let blank=c.makeImage()!
 var failure:Error?
 for attempt in 0..<3 {
  // A compile that timed out on the client side usually still completes in the
  // system service and is cached by the next attempt.
  if attempt>0 {Thread.sleep(forTimeInterval:5)}
  let request=VNRecognizeTextRequest();request.recognitionLevel = .accurate;request.recognitionLanguages=["en-US","zh-Hans"]
  do {try VNImageRequestHandler(cgImage:blank,options:[:]).perform([request]);return} catch {failure=error}
 }
 throw failure!
}
// A failure after warm-up means the cached models are unavailable. Rebuild them
// once under the lock rather than compiling at the same time as other helpers.
func recognize(_ request:VNRecognizeTextRequest,_ image:CGImage) throws {
 do {try VNImageRequestHandler(cgImage:image,options:[:]).perform([request])}
 catch {try prepareTextRecognition();try VNImageRequestHandler(cgImage:image,options:[:]).perform([request])}
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
  try prepareTextRecognition()
  var cues=[TextCue](),unreadable=0
  for (index,cue) in manifest.cues.enumerated() {
   guard cue.file == String(format:"%05d.gray",index), cue.width>0,cue.height>0,cue.width<=4096,cue.height<=2160,
         let end=cue.end_ms,end>cue.start_ms else {throw NSError(domain:"Invalid OCR cue",code:1)}
   let text=try autoreleasepool { () throws -> String in
    let bitmap=try image(cue,root)
    let request=VNRecognizeTextRequest();request.recognitionLevel = .accurate
    request.recognitionLanguages=hint == "en" ? ["en-US"] : hint == "zh-Hans" ? ["zh-Hans"] : ["en-US","zh-Hans"]
    request.usesLanguageCorrection=true
    try recognize(request,bitmap)
    if (request.results ?? []).isEmpty {
     let w=max(800,bitmap.width+100),h=max(300,bitmap.height+100)
     let c=CGContext(data:nil,width:w,height:h,bitsPerComponent:8,bytesPerRow:w,space:CGColorSpaceCreateDeviceGray(),bitmapInfo:0)!
     c.setFillColor(gray:1,alpha:1);c.fill(CGRect(x:0,y:0,width:w,height:h));c.draw(bitmap,in:CGRect(x:(w-bitmap.width)/2,y:(h-bitmap.height)/2,width:bitmap.width,height:bitmap.height))
     let padded=c.makeImage()!
     try recognize(request,padded)
     if (request.results ?? []).isEmpty {
      request.recognitionLevel = .fast
      // Vision's fast recognizer does not support every accurate-path language.
      request.recognitionLanguages=["en-US"]
      if hint != "zh-Hans" {try recognize(request,padded)}
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
