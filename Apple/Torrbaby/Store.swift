import Foundation
import SwiftUI
import Security
#if os(iOS) || os(tvOS)
import UIKit
#endif

struct APIError: LocalizedError {
    let message: String
    var errorDescription: String? { message }
}

enum Credentials {
    static let service = "org.torrbaby.client"
    static func read(account: String = "api-token") -> String {
        let query: [String: Any] = [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: service,
                                    kSecAttrAccount as String: account, kSecReturnData as String: true]
        var result: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess, let data = result as? Data else { return "" }
        return String(data: data, encoding: .utf8) ?? ""
    }
    static func save(_ token: String, account: String = "api-token") throws {
        let query: [String: Any] = [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: service, kSecAttrAccount as String: account]
        SecItemDelete(query as CFDictionary)
        var item = query
        item[kSecValueData as String] = Data(token.utf8)
        item[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        let status = SecItemAdd(item as CFDictionary, nil)
        guard status == errSecSuccess else { throw APIError(message: "Could not save the API token to Keychain (\(status)).") }
    }
}

@MainActor
final class Store: ObservableObject {
    @Published var library: MediaLibrary?
    @Published var libraryError: String?
    @Published var metadata: [MediaMetadata] = []
    @Published var transferHistory: [TransferSample] = []
    private var lastHistoryFetch = Date.distantPast
    @Published var clients: [ClientDevice] = []
    @Published var history: [PlaybackHistory] = []
    let clientId: String = {
        let existing = Credentials.read(account: "client-id")
        if UUID(uuidString: existing) != nil { return existing }
        let id = UUID().uuidString.lowercased()
        try? Credentials.save(id, account: "client-id")
        return id
    }()
    var defaultClientName: String {
        #if os(macOS)
        return Host.current().localizedName ?? ProcessInfo.processInfo.hostName
        #else
        return UIDevice.current.name
        #endif
    }
    var clientPlatform: String {
        #if os(macOS)
        return "macOS"
        #elseif os(tvOS)
        return "tvOS"
        #else
        return UIDevice.current.userInterfaceIdiom == .pad ? "iPadOS" : "iOS"
        #endif
    }
    func registerClient() async throws {
        let _: ClientDevice = try await request("POST", "/v1/clients", body: ["client_id": clientId, "name": defaultClientName, "platform": clientPlatform])
    }
    private var flushingProgress = false
    private var pendingKey: String { "pending-playback:" + server }
    func reportProgress(sessionId: String, sequence: Int, position: Double, duration: Double, ended: Bool) {
        guard position.isFinite, duration.isFinite, duration > 0 else { return }
        var pending = UserDefaults.standard.dictionary(forKey: pendingKey) ?? [:]
        pending[sessionId] = ["session_id": sessionId, "sequence": sequence, "position": position, "duration": duration, "ended": ended]
        UserDefaults.standard.set(pending, forKey: pendingKey)
        Task { await flushProgress() }
    }
    func flushProgress() async {
        guard !flushingProgress else { return }
        flushingProgress = true
        defer { flushingProgress = false }
        let key = pendingKey
        let pending = UserDefaults.standard.dictionary(forKey: key) ?? [:]
        for (id, value) in pending {
            guard key == pendingKey else { break }
            guard let body = value as? [String: Any] else { continue }
            do {
                try await send("POST", "/v1/playback/progress", body: body)
                var current = UserDefaults.standard.dictionary(forKey: key) ?? [:]
                if let report = current[id] as? [String: Any], report["sequence"] as? Int == body["sequence"] as? Int {
                    current.removeValue(forKey: id)
                    UserDefaults.standard.set(current, forKey: key)
                }
            } catch { break }
        }
    }
    @Published var server: String = UserDefaults.standard.string(forKey: "server") ?? ""
    @Published var token: String = Credentials.read()
    @Published var torrents: [Torrent] = []
    @Published var status: DaemonStatus?
    @Published var events: [AuditEvent] = []
    @Published var connectionError: String?
    @Published var actionError: String?
    @Published var busy = false
    @Published var lastUpdated: Date?
    @Published var isConnecting = false
    private var refreshing = false
    private var etaSmoother = TorrentETASmoother()
    init() {
        #if os(macOS)
        if server.isEmpty, let legacy = UserDefaults.standard.persistentDomain(forName: "org.torrbaby.app"), let address = legacy["server"] as? String {
            server = address
            UserDefaults.standard.set(address, forKey: "server")
            if let compact = legacy["compactTorrentView"] as? Bool { UserDefaults.standard.set(compact, forKey: "compactTorrentView") }
        }
        #endif
        #if DEBUG
        // Optional one-use profile for local simulator/UI smoke tests. No secrets in builds.
        if let path = ProcessInfo.processInfo.environment["TORRBABY_BOOTSTRAP_FILE"],
           let data = try? Data(contentsOf: URL(fileURLWithPath: path)),
           let profile = try? JSONDecoder().decode([String: String].self, from: data),
           let address = profile["server"], let secret = profile["token"] {
            server = address
            token = secret
            UserDefaults.standard.set(address, forKey: "server")
            try? Credentials.save(secret)
            try? FileManager.default.removeItem(atPath: path)
        }
        #endif
        #if os(macOS)
        // App-owned polling continues after the last library window is closed.
        Task { [weak self] in
            while !Task.isCancelled {
                await self?.refresh()
                let delay = self?.connectionError == nil ? 5 : 15
                try? await Task.sleep(for: .seconds(delay))
                if self == nil { break }
            }
        }
        #endif
    }
    var configured: Bool { !server.isEmpty && !token.isEmpty }
    var connected: Bool { status != nil && connectionError == nil }
    private let decoder: JSONDecoder = {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return decoder
    }()
    private let session: URLSession = {
        let config = URLSessionConfiguration.ephemeral
        config.timeoutIntervalForRequest = 25
        config.timeoutIntervalForResource = 45
        return URLSession(configuration: config)
    }()
    func endpoint(_ path: String) throws -> URL {
        guard let base = URL(string: server.trimmingCharacters(in: .whitespacesAndNewlines)),
              ["http", "https"].contains(base.scheme?.lowercased() ?? ""), base.host != nil,
              base.user == nil, base.password == nil, base.query == nil, base.fragment == nil,
              let url = URL(string: server.trimmingCharacters(in: CharacterSet(charactersIn: "/")) + path)
        else { throw APIError(message: "Enter a server URL such as http://your-server:8765.") }
        return url
    }
    func request<T: Decodable>(_ method: String, _ path: String, body: [String: Any]? = nil) async throws -> T {
        var request = URLRequest(url: try endpoint(path))
        request.httpMethod = method
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        if let body {
            request.httpBody = try JSONSerialization.data(withJSONObject: body)
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            let payload = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
            throw APIError(message: payload?["error"] as? String ?? "The server could not complete the request.")
        }
        return try decoder.decode(T.self, from: data)
    }
    struct Empty: Decodable {}
    func send(_ method: String = "POST", _ path: String, body: [String: Any] = [:]) async throws {
        let _: Empty = try await request(method, path, body: body)
    }
    func connect() async {
        isConnecting = true
        defer { isConnecting = false }
        do {
            server = server.trimmingCharacters(in: .whitespacesAndNewlines).trimmingCharacters(in: CharacterSet(charactersIn: "/"))
            token = token.trimmingCharacters(in: .whitespacesAndNewlines)
            let _: DaemonStatus = try await request("GET", "/v1/status")
            try Credentials.save(token)
            UserDefaults.standard.set(server, forKey: "server")
            await refresh()
        } catch { connectionError = error.localizedDescription }
    }
    func refresh() async {
        guard configured, !refreshing else { return }
        refreshing = true
        defer { refreshing = false }
        do {
            let newStatus: DaemonStatus = try await request("GET", "/v1/status")
            let received: [Torrent] = try await request("GET", "/v1/torrents")
            let newTorrents = etaSmoother.update(received, server: server, now: Date().timeIntervalSince1970)
            status = newStatus
            torrents = newTorrents.sorted { $0.addedDate > $1.addedDate }
            connectionError = nil
            let newEvents: [AuditEvent] = try await request("GET", "/v1/events")
            try await registerClient()
            clients = try await request("GET", "/v1/clients")
            history = try await request("GET", "/v1/playback")
            do {
                library = try await request("GET", "/v1/library")
                libraryError = nil
            } catch { libraryError = error.localizedDescription }
            if Date().timeIntervalSince(lastHistoryFetch) >= 30 {
                transferHistory = try await request("GET", "/v1/rates")
                metadata = try await request("GET", "/v1/metadata")
                lastHistoryFetch = Date()
            }
            await flushProgress()
            status = newStatus
            torrents = newTorrents.sorted { $0.addedDate > $1.addedDate }
            events = newEvents.reversed()
            lastUpdated = Date()
            connectionError = nil
        } catch { connectionError = error.localizedDescription }
    }
    func perform(_ operation: @escaping () async throws -> Void) {
        guard !busy else { return }
        busy = true
        Task {
            defer { busy = false }
            do { try await operation(); lastHistoryFetch = .distantPast; await refresh() }
            catch { actionError = error.localizedDescription }
        }
    }
    func action(_ torrent: Torrent, _ action: String, body: [String: Any] = [:]) {
        perform { try await self.send("POST", "/v1/torrents/\(torrent.id)/\(action)", body: body) }
    }
    func toggleAutomation() {
        guard let status else { return }
        perform { try await self.send("PATCH", "/v1/settings", body: ["paused": !status.settings.paused]) }
    }
}
