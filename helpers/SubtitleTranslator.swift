import Foundation
import Translation

// One bounded JSON request per process. Cue timing and persistent caching belong
// to Rust; this helper only bridges Apple's on-device translation framework.
private struct Cue: Codable {
    let id: String
    let text: String
}
private struct Request: Decodable {
    let source: String
    let target: String
    let cues: [Cue]
}
private struct Response: Encodable {
    let version = 1
    let cues: [Cue]?
    let error: String?
}
private enum Failure: String, Error {
    case invalidRequest = "invalid_request"
    case modelsNotInstalled = "models_not_installed"
    case translationFailed = "translation_failed"
    case invalidResponse = "invalid_response"
}

@main struct SubtitleTranslator {
    static func main() async {
        do {
            // Read one extra byte to detect oversized input without unbounded RAM.
            var data = Data()
            while data.count <= 1_048_576 {
                let chunk = try FileHandle.standardInput.read(upToCount: min(65_536, 1_048_577 - data.count)) ?? Data()
                if chunk.isEmpty { break }
                data.append(chunk)
            }
            guard data.count <= 1_048_576,
                  let request = try? JSONDecoder().decode(Request.self, from: data),
                  ["en", "zh-Hans"].contains(request.source),
                  ["en", "zh-Hans"].contains(request.target),
                  request.source != request.target,
                  !request.cues.isEmpty, request.cues.count <= 128,
                  Set(request.cues.map(\.id)).count == request.cues.count,
                  request.cues.allSatisfy({ !$0.id.isEmpty && $0.id.utf8.count <= 128
                      && !$0.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                      && $0.text.utf8.count <= 16_384 })
            else { throw Failure.invalidRequest }

            let source = Locale.Language(identifier: request.source)
            let target = Locale.Language(identifier: request.target)
            guard await LanguageAvailability().status(from: source, to: target) == .installed
            else { throw Failure.modelsNotInstalled }
            let session = TranslationSession(installedSource: source, target: target)
            let translated = try await session.translations(from: request.cues.map {
                TranslationSession.Request(sourceText: $0.text, clientIdentifier: $0.id)
            })
            let expected = Set(request.cues.map(\.id))
            var byID: [String: String] = [:]
            for cue in translated {
                guard let id = cue.clientIdentifier, expected.contains(id), byID[id] == nil,
                      !cue.targetText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                else { throw Failure.invalidResponse }
                byID[id] = cue.targetText
            }
            guard byID.count == request.cues.count else { throw Failure.invalidResponse }
            // Preserve request order even if the framework returns a reordered batch.
            emit(Response(cues: request.cues.map { Cue(id: $0.id, text: byID[$0.id]!) }, error: nil))
        } catch {
            // Never log cue content, filenames or framework error descriptions.
            emit(Response(cues: nil, error: (error as? Failure ?? .translationFailed).rawValue))
            exit(1)
        }
    }

    private static func emit(_ response: Response) {
        guard var data = try? JSONEncoder().encode(response) else { exit(2) }
        data.append(10)
        FileHandle.standardOutput.write(data)
    }
}
