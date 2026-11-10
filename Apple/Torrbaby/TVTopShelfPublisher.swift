#if os(tvOS)
import Foundation
import TVServices

@MainActor
enum TVTopShelfPublisher {
    static func clear() {
        TopShelfSnapshot.clear()
        TVTopShelfContentProvider.topShelfContentDidChange()
    }

    static func publish(_ store: Store) async {
        guard let profile = store.activeProfile, let library = store.library else { return }
        let eligible = library.titles.filter { $0.backdrop != nil && !$0.playable.isEmpty }
        let current = eligible.filter { $0.isContinuing }.max { $0.continueWatchingAt < $1.continueWatchingAt }
            ?? eligible.filter { $0.continueWatchingAt > 0 }.max { $0.continueWatchingAt < $1.continueWatchingAt }
        let candidates = ([current].compactMap { $0 } + eligible.filter { $0.id != current?.id }.sorted {
            $0.addedAt == $1.addedAt ? $0.id < $1.id : $0.addedAt > $1.addedAt
        }).prefix(12)
        var entries = [TopShelfSnapshot.Item]()
        for title in candidates {
            guard let raw = title.backdrop, let image = URL(string: raw), image.scheme == "https", image.host != nil else { continue }
            func link(_ action: String) -> URL {
                var url = URLComponents()
                url.scheme = "torrbaby"; url.host = "library"
                url.queryItems = [URLQueryItem(name: "title", value: title.id), URLQueryItem(name: "profile", value: profile.id), URLQueryItem(name: "action", value: action)]
                return url.url!
            }
            let context = title.id == current?.id
                ? L10n.text(title.isContinuing ? "Continue watching" : "Recently watched") : L10n.text("Recently added")
            entries.append(.init(id: title.id, title: title.title, context: context, summary: title.summary,
                                 image: image, detailURL: link("detail"), playURL: link("play")))
            if entries.count == 5 { break }
        }
        let snapshot = TopShelfSnapshot(profile: profile.id, items: entries)
        guard snapshot != TopShelfSnapshot.read() else { return }
        if snapshot.save() { TVTopShelfContentProvider.topShelfContentDidChange() }
    }
}
#endif
