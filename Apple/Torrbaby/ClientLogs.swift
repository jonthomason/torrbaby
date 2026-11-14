import Foundation
import OSLog

/// A running account of what this device did that matters when something goes wrong away
/// from home: each playback (opening, waiting, stalls, AVFoundation's own access and error
/// logs) and other notes. Kept in a small file so it survives the app being closed or killed,
/// and sent with Settings → Connection → Send logs to Diamond.
enum ClientDiagnostics {
    private static let lock = NSLock()
    private static let limit = 512 * 1024
    private static let clock: DateFormatter = {
        let f = DateFormatter(); f.dateFormat = "yyyy-MM-dd HH:mm:ss.SSS"; f.locale = Locale(identifier: "en_US_POSIX"); return f
    }()
    private static var file: URL? {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first?
            .appendingPathComponent("Torrbaby", isDirectory: true).appendingPathComponent("client-diagnostics.log")
    }

    static func note(_ line: String) {
        guard let file else { return }
        let text = "\(clock.string(from: Date())) \(line)\n"
        lock.lock(); defer { lock.unlock() }
        try? FileManager.default.createDirectory(at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
        if let handle = try? FileHandle(forWritingTo: file) {
            defer { try? handle.close() }
            _ = try? handle.seekToEnd()
            try? handle.write(contentsOf: Data(text.utf8))
            // Keep the newest half once it grows past the limit.
            if let size = try? handle.offset(), size > limit, let data = try? Data(contentsOf: file) {
                var kept = data.suffix(limit / 2)
                if let cut = kept.firstIndex(of: 0x0A) { kept = kept.suffix(from: kept.index(after: cut)) }
                try? kept.write(to: file, options: .atomic)
            }
        } else {
            try? Data(text.utf8).write(to: file, options: .atomic)
        }
    }

    static func read() -> String {
        guard let file else { return "" }
        lock.lock(); defer { lock.unlock() }
        return (try? String(contentsOf: file, encoding: .utf8)) ?? ""
    }
}

enum ClientLogs {
    /// Settings → Connection → Send logs automatically: this device's own choice, off at first.
    static let automaticKey = "clientLogs.automatic"
    static var automatic: Bool { UserDefaults.standard.bool(forKey: automaticKey) }
    @MainActor private static weak var store: Store?
    @MainActor private static var lastSent = Date.distantPast
    @MainActor private static var lastSentLength = 0
    @MainActor private static var pending: Task<Void, Never>?
    @MainActor private static var sending = false

    /// While the app runs: with the setting on, sends every 15 minutes when there is something
    /// new, and soon after a failure (see `failed`), including the tunnel disconnecting.
    @MainActor static func runAutomatic(_ store: Store) async {
        self.store = store
        #if os(iOS)
        var disconnect = PacketTunnel.shared.lastDisconnect
        #endif
        var tick = 0
        while !Task.isCancelled {
            try? await Task.sleep(for: .seconds(30))
            tick += 1
            guard automatic else { continue }
            #if os(iOS)
            let now = PacketTunnel.shared.lastDisconnect
            if now != disconnect, let now { disconnect = now; failed("Tunnel disconnected: \(now)") }
            #endif
            if tick % 30 == 0 { await sendIfNew(reason: "periodic") }
        }
    }

    /// A failure worth having on Diamond: noted, then (with the setting on) the logs go a minute
    /// later, so what followed is included; at most one such send every five minutes.
    nonisolated static func failed(_ what: String) {
        ClientDiagnostics.note("Failure: \(what)")
        Task { @MainActor in
            guard automatic, pending == nil, Date().timeIntervalSince(lastSent) > 300 else { return }
            pending = Task { @MainActor in
                try? await Task.sleep(for: .seconds(60))
                pending = nil
                await sendIfNew(reason: "after a failure")
            }
        }
    }

    @MainActor private static func sendIfNew(reason: String) async {
        guard let store, store.connected, !sending else { return }
        let length = ClientDiagnostics.read().utf8.count + tunnelLength
        guard length != lastSentLength else { return }
        sending = true; defer { sending = false }
        do {
            let name = try await send(store: store, report: "Sent automatically (\(reason))\n" + report(store))
            lastSent = Date(); lastSentLength = length
            ClientDiagnostics.note("Logs sent to Diamond automatically (\(reason)): \(name)")
        } catch { ClientDiagnostics.note("Automatic log upload failed: \(error.localizedDescription)") }
    }
    private static var tunnelLength: Int {
        #if os(iOS)
        TunnelLog.read(100_000).reduce(0) { $0 + $1.utf8.count }
        #else
        0
        #endif
    }

    /// The connection's state, for uploads made without the Connection page open.
    @MainActor static func report(_ store: Store) -> String {
        var lines = ["Connected by: \(store.path.rawValue)", "Route: \(store.activeServer ?? "none")"]
        for (kind, check) in store.routeChecks.sorted(by: { $0.key < $1.key }) {
            lines.append("\(kind): \(check.ok ? "ok" : "failed") at \(check.at.formatted(date: .omitted, time: .standard)), \(check.detail)\(check.ms.map { ", \($0) ms" } ?? "")")
        }
        #if os(iOS)
        lines.append(store.directDiagnostics)
        lines.append("Torrbaby tunnel: \(PacketTunnel.shared.statusText)" + (PacketTunnel.shared.lastDisconnect.map { ", last disconnect: \($0)" } ?? ""))
        lines.append("Events:")
        lines += store.directEvents.map { "  \($0)" }
        #endif
        return lines.joined(separator: "\n")
    }

    /// This run of the app's own log (Torrbaby's subsystem), the last two hours.
    private static func systemLog() -> String {
        do {
            let store = try OSLogStore(scope: .currentProcessIdentifier)
            let since = store.position(date: Date().addingTimeInterval(-2 * 3600))
            let format = DateFormatter(); format.dateFormat = "HH:mm:ss.SSS"
            let lines = try store.getEntries(at: since, matching: NSPredicate(format: "subsystem BEGINSWITH %@", "org.torrbaby"))
                .compactMap { $0 as? OSLogEntryLog }
                .map { "\(format.string(from: $0.date)) [\($0.category)] \($0.composedMessage)" }
            return lines.suffix(3000).joined(separator: "\n")
        } catch { return "(system log unavailable: \(error.localizedDescription))" }
    }

    /// Everything worth reading about this device's connection and playback, as one text.
    @MainActor static func collect(store: Store, report: String) -> String {
        var sections = [
            "Torrbaby client logs from \(store.defaultClientName) (\(store.clientPlatform)), \(AppRelease.description), sent \(Date().formatted(.iso8601))",
            "== Connection report ==\n" + report,
        ]
        #if os(iOS)
        sections.append("== Tunnel extension log ==\n" + TunnelLog.read(100_000).joined(separator: "\n"))
        #endif
        sections.append("== Playback and app notes ==\n" + ClientDiagnostics.read())
        sections.append("== App system log (this run) ==\n" + systemLog())
        return sections.joined(separator: "\n\n") + "\n"
    }

    /// Sends the logs in parts small enough for the server's request limit; returns the name
    /// Diamond saved them under.
    @MainActor static func send(store: Store, report: String) async throws -> String {
        let text = collect(store: store, report: report)
        let upload = UUID().uuidString.lowercased()
        var parts: [Substring] = []
        var rest = Substring(text)
        // Split on characters, about 24 KB each: JSON escaping can nearly double some text.
        while !rest.isEmpty {
            var end = rest.index(rest.startIndex, offsetBy: 24_000, limitedBy: rest.endIndex) ?? rest.endIndex
            if end != rest.endIndex, let newline = rest[..<end].lastIndex(of: "\n") { end = rest.index(after: newline) }
            parts.append(rest[..<end]); rest = rest[end...]
        }
        struct Saved: Decodable { let saved: String? }
        var name = ""
        for (index, part) in parts.enumerated() {
            let saved: Saved = try await store.request("POST", "/v1/diagnostics/logs", body: [
                "upload": upload, "part": index, "device": store.defaultClientName, "text": String(part), "last": index == parts.count - 1,
            ], timeout: 30)
            name = saved.saved ?? name
        }
        return name
    }
}
