import Foundation
import TVServices

final class ContentProvider: TVTopShelfContentProvider {
    override func loadTopShelfContent(completionHandler: @escaping (TVTopShelfContent?) -> Void) {
        guard let snapshot = TopShelfSnapshot.read() else { completionHandler(nil); return }
        let items = snapshot.items.prefix(5).compactMap { entry -> TVTopShelfCarouselItem? in
            guard entry.image.scheme == "https" else { return nil }
            let image = entry.image
            let item = TVTopShelfCarouselItem(identifier: entry.id)
            item.title = entry.title
            item.contextTitle = entry.context
            item.summary = entry.summary
            item.setImageURL(image, for: [.screenScale1x, .screenScale2x])
            item.displayAction = TVTopShelfAction(url: entry.detailURL)
            item.playAction = TVTopShelfAction(url: entry.playURL)
            // Stills only. The system owns carousel timing and transitions.
            return item
        }
        completionHandler(items.isEmpty ? nil : TVTopShelfCarouselContent(style: .details, items: items))
    }
}
