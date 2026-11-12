#if !os(tvOS)
import SwiftUI

/// The shows the server watches for new episodes, as it reports them.
struct WatchList: Decodable {
    struct Season: Decodable { let season: Int; let episodes: [Int] }
    struct Show: Decodable, Identifiable {
        let id: String
        let title: String?
        let originalTitle: String?
        let year: String?
        var market: String
        let auto: Bool
        let airing: Bool
        let missing: [Season]
        let checked: Double?
        let found: Int?
        let error: String?
        var name: String { [title ?? "", originalTitle ?? ""].filter { !$0.isEmpty }.reduce(into: [String]()) { if !$0.contains($1) { $0.append($1) } }.joined(separator: " · ") }
        /// "Season 1: E37–E40", or that nothing is missing.
        var missingLine: String {
            missing.isEmpty ? L10n.text("Up to date") : L10n.text("Missing") + " " + missing.map { season in
                (missing.count > 1 || season.season > 1 ? L10n.format("Season %d", season.season) + " " : "") + CloudCopy.ranges(season.episodes)
            }.joined(separator: " · ")
        }
    }
    let shows: [Show]
    let nextCheck: Double?
    let checking: Bool?
}

/// Shows watched for new episodes: where each is looked for, what it lacks, and when it
/// was last checked. The server checks twice a day; airing shows are added by themselves.
struct WatchingView: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    @State private var list: WatchList?
    @State private var error: String?
    private func time(_ seconds: Double) -> String { Date(timeIntervalSince1970: seconds).formatted(date: .abbreviated, time: .shortened) }
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                HStack(alignment: .firstTextBaseline) {
                    VStack(alignment: .leading, spacing: 4) {
                        if let next = list?.nextCheck { Text(L10n.format("Next check: %@", time(next))).font(.subheadline).foregroundStyle(Theme.muted) }
                        Text("Missing episodes are looked for around midday and in the evening.").font(.caption).foregroundStyle(Theme.muted)
                    }.textSelectionIfAvailable()
                    Spacer()
                    if list?.checking == true { ProgressView().controlSize(.small) }
                    Button("Check now") { act("POST", "/v1/watch/check") }.buttonStyle(MonoButtonStyle(compact: true)).fixedSize().disabled(list?.checking == true || list?.shows.isEmpty != false)
                }
                if let error { Text(verbatim: error).font(.callout).foregroundStyle(.orange) }
                if let list, list.shows.isEmpty {
                    ContentUnavailableView("No shows are being watched", systemImage: "eye", description: Text("Shows that are airing now are added by themselves. Turn on Watch for new episodes on any show’s page to add another."))
                        .padding(.vertical, 40)
                }
                ForEach(list?.shows ?? []) { show in row(show) }
            }.padding(24).frame(maxWidth: 1000).frame(maxWidth: .infinity)
        }
        .navigationTitle("Watching")
        .task(id: store.server) {
            while !Task.isCancelled {
                await load()
                try? await Task.sleep(for: .seconds(list?.checking == true ? 4 : 30))
            }
        }
    }
    private func row(_ show: WatchList.Show) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline) {
                Text(verbatim: show.name).font(.headline).lineLimit(2)
                Spacer(minLength: 8)
                // Where its episodes are looked for: cloud-drive shares, or torrents.
                Picker("Source", selection: Binding(get: { show.market }, set: { market in act("POST", "/v1/watch/\(show.id)", body: ["market": market]) })) {
                    Text("World").tag("world")
                    Text("China").tag("china")
                }.pickerStyle(.segmented).labelsHidden().fixedSize()
                Button("Stop watching") { act("DELETE", "/v1/watch/\(show.id)") }.buttonStyle(MonoButtonStyle(compact: true)).fixedSize()
            }
            Text(verbatim: show.missingLine).font(.subheadline).monospacedDigit().foregroundStyle(show.missing.isEmpty ? Theme.accent : Color.orange)
            HStack(spacing: 8) {
                if show.airing { Text("Airing now") }
                if show.auto { Text("Added automatically") }
                if let checked = show.checked {
                    Text(L10n.format("Last checked %@", time(checked)))
                    if let found = show.found, found > 0 { Text(L10n.format("Queued: %d", found)) }
                }
            }.font(.caption).foregroundStyle(Theme.muted)
            if let problem = show.error, !problem.isEmpty { Text(verbatim: problem).font(.caption).foregroundStyle(.orange) }
        }
        .textSelectionIfAvailable()
        .padding(12).frame(maxWidth: .infinity, alignment: .leading).background(Theme.panel, in: RoundedRectangle(cornerRadius: 10))
    }
    private func load() async {
        do { list = try await store.request("GET", "/v1/watch", timeout: 20); error = nil }
        catch { if list == nil { self.error = error.localizedDescription } }
    }
    private func act(_ method: String, _ path: String, body: [String: Any]? = nil) {
        Task {
            struct Ignored: Decodable {}
            do { let _: Ignored = try await store.request(method, path, body: body, timeout: 20) } catch { self.error = error.localizedDescription }
            await load()
        }
    }
}

/// The switch on a show's page.
struct WatchToggle: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    let title: LibraryTitle
    @State private var on = false
    @State private var known = false
    var body: some View {
        Toggle(isOn: Binding(get: { on }, set: { value in on = value; set(value) })) {
            Label("Watch for new episodes", systemImage: "eye")
        }
        .font(.subheadline).tint(.green).disabled(!known)
        .task(id: title.id) {
            let list: WatchList? = try? await store.request("GET", "/v1/watch", timeout: 20)
            if let list { on = list.shows.contains { $0.id == title.id }; known = true }
        }
    }
    private func set(_ value: Bool) {
        Task {
            struct Ignored: Decodable {}
            do {
                if value { let _: Ignored = try await store.request("POST", "/v1/watch", body: ["id": title.id], timeout: 20) }
                else { let _: Ignored = try await store.request("DELETE", "/v1/watch/\(title.id)", timeout: 20) }
            } catch { on = !value; store.actionError = error.localizedDescription }
        }
    }
}
#endif
