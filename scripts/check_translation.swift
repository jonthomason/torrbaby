import Foundation
import Translation
@main struct Probe {
 static func main() async {
  let availability = LanguageAvailability()
  let en=Locale.Language(identifier:"en"), zh=Locale.Language(identifier:"zh-Hans")
  for (source,target) in [(en,zh),(zh,en)] {
   let status=await availability.status(from:source,to:target)
   print("\(source.minimalIdentifier) -> \(target.minimalIdentifier): \(status)")
   if status == .installed {
    do {
     let session=TranslationSession(installedSource:source,target:target)
     let result=try await session.translate(source.minimalIdentifier == "en" ? "Where are we going?" : "我们要去哪里？")
     print("Translation: \(result.targetText)")
    } catch { print("Translation failed: \(error)") }
   }
  }
 }
}
