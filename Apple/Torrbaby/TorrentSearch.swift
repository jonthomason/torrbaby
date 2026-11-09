import SwiftUI

struct TorrentSearchSource: Decodable {
    let provider: String
    let id: String
    let url: String
    var label: String { provider == "tpb" ? "The Pirate Bay" : provider == "csv" ? "Torrent CSV" : "Nyaa" }
}
struct TorrentReleaseHints: Decodable {
    let year: Int?
    let season: Int?
    let episode: Int?
    let resolution: String?
    let videoCodec: String?
    let releaseSource: String?
    let audio: [String]
    let languages: [String]
    var summary: String { ([resolution, videoCodec, releaseSource].compactMap { $0 } + audio + languages).joined(separator: " · ") }
}
struct TorrentSearchItem: Decodable, Identifiable {
    let infoHash: String
    let name: String
    let magnet: String
    let sizeBytes: UInt64?
    let sizeLabel: String?
    let seeders: UInt64?
    let leechers: UInt64?
    let sources: [TorrentSearchSource]
    let hints: TorrentReleaseHints
    let matchScore: Int
    let matchNotes: [String]
    var id: String { infoHash }
    var size: String { sizeBytes.map { ByteCountFormatter.string(fromByteCount: Int64(clamping: $0), countStyle: .file) } ?? sizeLabel ?? "" }
}
private struct TorrentSearchResponse: Decodable {
    struct Provider: Decodable { let provider: String; let count: Int; let error: String? }
    let items: [TorrentSearchItem]
    let providers: [Provider]
}
private struct TorrentSearchDetails: Decodable {
    struct File: Decodable { let name: String; let sizeBytes: UInt64?; let hints: TorrentReleaseHints }
    let description: String
    let files: [File]?
    let audioLanguage: String?
    let subtitleLanguage: String?
    enum CodingKeys: String, CodingKey { case description, files, audioLanguage, subtitleLanguage }
    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        description = try values.decode(String.self, forKey: .description)
        files = try values.decodeIfPresent([File].self, forKey: .files)
        audioLanguage = try? values.decode(String.self, forKey: .audioLanguage)
        subtitleLanguage = try? values.decode(String.self, forKey: .subtitleLanguage)
    }
    var languages: [String] {
        [("Audio", audioLanguage), ("Subtitles", subtitleLanguage)].compactMap { label, value in
            guard let value = value?.trimmingCharacters(in: .whitespacesAndNewlines),
                  !value.isEmpty, !["unknown", "und", "0"].contains(value.lowercased()) else { return nil }
            return "\(label) · \(value)"
        }
    }
}

struct TorrentSearchView: View {
    @EnvironmentObject var store: Store
    @State private var query: String
    @State private var year: Int?
    @State private var season: Int?
    @State private var episode: Int?
    init(initialQuery: String = "", year: Int? = nil, season: Int? = nil, episode: Int? = nil) {
        _query = State(initialValue: initialQuery); _year = State(initialValue: year)
        _season = State(initialValue: season); _episode = State(initialValue: episode)
    }
    @State private var source = "all"
    @State private var response: TorrentSearchResponse?
    @State private var error: String?
    @State private var searching = false
    @State private var work: Task<Void, Never>?
    var body: some View {
        VStack(spacing: 0) {
            HStack {
                TextField("Search torrents", text: $query).onSubmit(search)
                    .autocorrectionDisabled()
                Button(action: search) { Label("Search", systemImage: "magnifyingglass") }
                    .disabled(query.trimmingCharacters(in: .whitespacesAndNewlines).count < 2 || searching)
            }.padding()
            Picker("Sources", selection: $source) {
                Text("Recommended").tag("all")
                Text("The Pirate Bay").tag("tpb")
                Text("Torrent CSV").tag("csv")
                Text("Nyaa").tag("nyaa")
            }.pickerStyle(.segmented).padding(.horizontal).padding(.bottom)
            if year != nil || season != nil || episode != nil {
                HStack {
                    Text([year.map { "Year \($0)" }, season.map { "Season \($0)" }, episode.map { "Episode \($0)" }].compactMap { $0 }.joined(separator: " · ")).foregroundStyle(.secondary)
                    Button("Clear filters") { year = nil; season = nil; episode = nil }
                }.font(.caption).padding(.bottom)
            }
            if searching { ProgressView().padding() }
            if let error { Text(error).foregroundStyle(.orange).padding() }
            if let response {
                ForEach(response.providers.filter { $0.error != nil }, id: \.provider) { provider in
                    Text("\(provider.provider == "tpb" ? "The Pirate Bay" : provider.provider == "csv" ? "Torrent CSV" : "Nyaa"): \(provider.error ?? "")")
                        .font(.caption).foregroundStyle(.secondary)
                }
                List(response.items) { item in
                    NavigationLink {
                        TorrentSearchDetail(item: item)
                    } label: {
                        VStack(alignment: .leading, spacing: 8) {
                            Text(item.name).font(.headline).lineLimit(3)
                            HStack {
                                Text(item.size)
                                if let seeders = item.seeders { Label("\(seeders)", systemImage: "arrow.up.circle").foregroundStyle(.green) }
                                if let peers = item.leechers { Label("\(peers)", systemImage: "arrow.down.circle").foregroundStyle(.secondary) }
                                Spacer()
                                Text(item.sources.map(\.label).joined(separator: " · ")).foregroundStyle(.secondary)
                            }.font(.caption).monospacedDigit()
                            if !item.hints.summary.isEmpty { Text(item.hints.summary).font(.caption).foregroundStyle(.secondary).lineLimit(2) }
                        }.padding(.vertical, 8)
                    }
                }
                if response.items.isEmpty && !searching {
                    ContentUnavailableView("No results", systemImage: "magnifyingglass", description: Text("Try a shorter title or another source."))
                }
            } else if !searching {
                ContentUnavailableView("Find a torrent", systemImage: "magnifyingglass")
            }
        }
        .navigationTitle("Find torrents")
        .onDisappear { work?.cancel(); searching = false }
        .task { if !query.isEmpty && response == nil { search() } }
    }
    private func search() {
        guard query.trimmingCharacters(in: .whitespacesAndNewlines).count >= 2 else { return }
        work?.cancel(); searching = true; error = nil
        var body: [String: Any] = ["query": query, "sources": source == "all" ? [] : [source]]
        if let year { body["year"] = year }; if let season { body["season"] = season }; if let episode { body["episode"] = episode }
        work = Task { @MainActor in
            do {
                let value: TorrentSearchResponse = try await store.request("POST", "/v1/search/torrents", body: body, timeout: 20)
                guard !Task.isCancelled else { return }; response = value
            } catch {
                guard !Task.isCancelled else { return }; self.error = error.localizedDescription
            }
            searching = false
        }
    }
}

private struct TorrentSearchDetail: View {
    @EnvironmentObject var store: Store
    let item: TorrentSearchItem
    @State private var detail: TorrentSearchDetails?
    @State private var loading = false
    @State private var adding = false
    @State private var paused = false
    @State private var added = false
    @State private var message: String?
    @State private var detailError: String?
    var body: some View {
        List {
            SwiftUI.Section {
                Text(item.name).font(.title2.bold()).searchSelectable
                Text(item.size).foregroundStyle(.secondary)
                Toggle("Add paused", isOn: $paused).disabled(adding || added)
                Button { Task { await add() } } label: {
                    Label(added ? "Added to Transmission" : adding ? "Adding…" : "Add torrent", systemImage: added ? "checkmark.circle.fill" : "plus.circle.fill")
                }.disabled(adding || added)
                if let message { Text(message).foregroundStyle(.secondary) }
            }
            if !item.hints.summary.isEmpty {
                SwiftUI.Section("Release tags") { Text(item.hints.summary) }
            }
            SwiftUI.Section("Sources") {
                ForEach(item.sources, id: \.url) { source in
                    #if os(tvOS)
                    Text(source.label)
                    #else
                    Link(source.label, destination: URL(string: source.url)!)
                    #endif
                }
            }
            if loading { ProgressView("Loading details…") }
            if let detailError { Text(detailError).foregroundStyle(.secondary) }
            if let detail {
                if !detail.languages.isEmpty {
                    SwiftUI.Section("Source languages") {
                        ForEach(detail.languages, id: \.self) { Text($0) }
                    }
                }
                if !detail.description.isEmpty { SwiftUI.Section("Source description") { Text(detail.description).searchSelectable } }
                if let files = detail.files {
                    SwiftUI.Section("Files · \(files.count)") {
                        ForEach(Array(files.enumerated()), id: \.offset) { _, file in
                            VStack(alignment: .leading, spacing: 5) {
                                Text(file.name).lineLimit(3)
                                if !file.hints.summary.isEmpty { Text(file.hints.summary).font(.caption).foregroundStyle(.secondary) }
                                if let size = file.sizeBytes { Text(ByteCountFormatter.string(fromByteCount: Int64(clamping: size), countStyle: .file)).font(.caption).foregroundStyle(.secondary) }
                            }
                        }
                    }
                }
            }
        }.navigationTitle("Torrent details")
        .task {
            guard let source = item.sources.first(where: { $0.provider == "tpb" }) ?? item.sources.first(where: { $0.provider == "nyaa" }) ?? item.sources.first else { return }
            loading = true; defer { loading = false }
            do { detail = try await store.request("POST", "/v1/search/torrents/detail", body: ["provider": source.provider, "id": source.id], timeout: 30) }
            catch { if !Task.isCancelled { detailError = "Couldn’t load source details. You can still add this torrent." } }
        }
    }
    private func add() async {
        guard !adding && !added else { return }; adding = true; message = nil
        defer { adding = false }
        do {
            struct Torrent: Decodable { let name: String?; let hashString: String? }
            struct Added: Decodable { let torrentAdded: Torrent?; let torrentDuplicate: Torrent? }
            let result: Added = try await store.request("POST", "/v1/torrents", body: ["magnet": item.magnet, "paused": paused], timeout: 45)
            added = true; message = result.torrentDuplicate == nil ? "Torrent added" : "Already in your torrents"
            store.notify(message ?? "Torrent added", detail: item.name, torrentHash: item.infoHash)
        } catch { message = error.localizedDescription }
    }
}

private extension View {
    @ViewBuilder var searchSelectable: some View {
        #if os(tvOS)
        self
        #else
        self.textSelection(.enabled)
        #endif
    }
}
