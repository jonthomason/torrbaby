import SwiftUI
#if os(iOS)
import UIKit
#elseif os(macOS)
import AppKit
#endif

/// Settings → Connection: how this device reaches the server, each way there and how it last
/// did, and on iPhone/iPad the direct path (hole punch) and the relay through bronze in
/// detail, with why there is no direct path when there is none. Technical text, kept in
/// English (agreed for this page); "Copy report" puts all of it on the clipboard.
struct ConnectionStatusSections: View {
    @EnvironmentObject var store: Store
    @State private var measuring = false
    /// Diamond's record of punches by carrier and connection kind (GET /v1/punch-results).
    @State private var punchResults: [PunchResult] = []
    struct PunchResult: Decodable, Hashable { let carrier: String; let kind: String; let punched: Int; let failed: Int; let medianMs: Int?; let failedBySpread: [String: Int]? }
    private struct PunchResults: Decodable { let results: [PunchResult] }
    /// bronze's account of the relay (GET /v1/relay-usage): devices on it now, bytes per day.
    @State private var relayUsage: RelayUsage?
    struct RelayUsage: Decodable {
        struct Device: Decodable, Hashable { let client: String; let name: String?; let address: String; let since: Double; let down: Int64; let up: Int64; let streams: Int64 }
        struct Day: Decodable { let down: Int64; let up: Int64 }
        let devices: [Device]; let days: [String: Day]; let today: String
    }
    @AppStorage(ClientLogs.automaticKey) private var automaticLogs = false
    @State private var sendingLogs = false
    /// What became of the last "Send logs to Diamond": the file's name there, or why it failed.
    @State private var logResult: String?
    #if os(iOS)
    @ObservedObject private var tunnel = PacketTunnel.shared
    /// The extension's own account of its paths, asked each second while the page is open.
    @State private var tunnelState: String?
    #endif

    /// Ticks every second so times and countdowns stay current. (A TimelineView around the
    /// sections merged them into one in a Form.)
    @State private var tick = 0

    var body: some View {
        // Reading the tick redraws the page each second in place. Giving the sections a new
        // identity instead (.id) rebuilt them and snapped the scroll position back to the top.
        let _ = tick
        Group {
            nowSection
            pathSections
            actionsSection
            punchResultsSection
            relayUsageSection
            #if os(iOS)
            tunnelSection
            directSection
            eventsSection
            #endif
        }
        .task { await loadPunchResults() }
        .task {
            // Every 10 s while the page is open: bytes move while video plays.
            while !Task.isCancelled {
                await loadRelayUsage()
                try? await Task.sleep(for: .seconds(10))
            }
        }
        .task {
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(1))
                tick &+= 1
                #if os(iOS)
                tunnelState = tunnel.connected ? await tunnel.ask() : nil
                #endif
            }
        }
    }

    private var nowSection: some View {
        SwiftUI.Section {
            HStack(spacing: 12) {
                Image(systemName: symbol(store.path)).font(.title2).foregroundStyle(color(store.path)).frame(width: 32)
                VStack(alignment: .leading, spacing: 2) {
                    Text(verbatim: store.path.rawValue).font(.title3.weight(.semibold))
                    if !summary.isEmpty { Text(verbatim: summary).font(.subheadline).foregroundStyle(.secondary) }
                }
            }.padding(.vertical, 4)
            #if os(iOS)
            // Only where the relay matters: carrying traffic, or the way back when nothing else answers.
            if [.relay, .direct, .offline].contains(store.path) {
                LabeledContent { Text(verbatim: relayReachable) } label: { Text(verbatim: "Relay") }
            }
            #endif
        } header: { Text(verbatim: "Connected by") } footer: {
            // Which builds are running: the first thing to know when reading a screenshot.
            Text(verbatim: versions).font(.caption.monospaced())
        }
    }

    /// "App 1.0 (275) · Tunnel 1.0 (275)": the tunnel's own, as it reports it (a tunnel left
    /// running from before an update is still the older build).
    private var versions: String {
        var text = "App " + AppRelease.short
        #if os(iOS)
        if let version = tunnelVersion { text += " · Tunnel \(version)" }
        else if tunnel.installed { text += " · Tunnel not running" }
        #endif
        return text
    }

    #if os(iOS)
    private var tunnelVersion: String? {
        tunnelState?.split(separator: "\n").first.flatMap { $0.hasPrefix("version ") ? String($0.dropFirst(8)) : nil }
    }
    private var tunnelPaths: String? {
        tunnelState.map { $0.split(separator: "\n").filter { !$0.hasPrefix("version ") }.joined(separator: "\n") }
    }
    /// "open" or "closed", from the tunnel's own report.
    private var tunnelRelay: String? {
        tunnelState?.split(separator: "\n").first { $0.hasPrefix("Relay via bronze: ") }.map { String($0.dropFirst("Relay via bronze: ".count)) }
    }
    private var relayReachable: String {
        if tunnel.connected, let state = tunnelRelay { return state == "open" ? "reachable (in the tunnel)" : "\(state) (in the tunnel)" }
        if let connected = store.relayServerConnected {
            return (store.linkRoute != nil || tunnel.connected ? "reachable" : "closed") + (connected ? ", \(server) connected" : ", \(server) NOT connected")
        }
        return store.relayAddress.isEmpty ? "unknown" : "not checked yet"
    }
    #endif

    #if os(iOS)
    /// ABT's way to Diamond in words, with how long ago it last answered when that is a while.
    private var abtDetail: String {
        guard let path = store.abtPath else { return "not known yet" }
        var text: String
        switch path {
        case "lan": text = "direct (home network)"
        case "punched": text = "direct (punched)"
        case "ipv6": text = "direct (IPv6)"
        case "punching": text = "punching…" + (ABT.punching.map { " \($0)" } ?? "")
        case "checking": text = "network changed, checking…"
        case "lost": text = "punched path closed; the relay is used until ABT punches again"
        case "bronze": text = "through bronze (the app's own relay is used)"
        case "exit": text = "through shadow only (not used for Diamond; the relay is used)"
        default: text = "none yet (the relay is used; never through ABT's exit)"
        }
        if let at = ABT.answeredAt, store.abtDirect || path == "bronze" {
            let age = Int(Date().timeIntervalSince(at))
            if age > 10 { text += " · last answered \(age) s ago" }
        }
        return text
    }
    #endif
    private var summary: String {
        switch store.path {
        case .home: return ms("home")
        case .tunnel: return ms("tunnel")
        case .abt: return ms("abt")
        case .direct:
            #if os(iOS)
            return store.linkState?.rttMs.map { "\($0) ms" } ?? ms("direct")
            #else
            return ""
            #endif
        case .relay: return ms("relay")
        case .tailscale: return ms("tailscale")
        // Connected, but the route in use has closed: the next check finds another way.
        case .offline: return store.connected ? "the way in use closed; finding another" : store.offlineReason ?? store.connectionError ?? ""
        }
    }

    @ViewBuilder private var pathSections: some View {
        SwiftUI.Section {
            row("Home network", kind: "home", inUse: store.path == .home, detail: store.homeRoute.isEmpty ? store.primaryAddress : store.homeRoute)
            #if os(iOS)
            row("Torrbaby tunnel", kind: "tunnel", inUse: store.path == .tunnel, detail: tunnel.installed ? tunnel.statusText + (tunnel.connected ? (tunnelPaths.map { "\n\($0)" } ?? "") : "") : "not installed")
            if ABT.connected || store.path == .abt {
                row("ABT", kind: "abt", inUse: store.path == .abt, detail: ABT.connected ? "on; its path to \(server): " + abtDetail : "off")
                if ABT.connected && store.abtIntercepted {
                    Text("This Wi-Fi is intercepting connections. Sign in again.").foregroundStyle(.red)
                }
            }
            // While the tunnel is connected the app's own direct path and relay stand down: their
            // old checks would read as failures, so they say so instead, and the relay line shows
            // the tunnel's own relay.
            if store.path == .abt {
                row("Torrbaby link", kind: "relay", inUse: false, detail: "not used: ABT carries traffic", showCheck: false)
            } else if tunnel.connected {
                row("Torrbaby link", kind: "relay", inUse: false, detail: "in the Torrbaby tunnel: " + (tunnelPaths ?? "state not known yet").replacingOccurrences(of: "\n", with: " · "), showCheck: false)
            } else {
                row("Torrbaby link", kind: store.path == .direct ? "direct" : "relay", inUse: store.path == .direct || store.path == .relay, detail: linkDetail)
            }
            #endif
            if store.tailscaleInUse {
                row("Tailscale", kind: "tailscale", inUse: store.path == .tailscale, detail: store.awayRoute.isEmpty ? (store.fallbackServer.isEmpty ? "not set up" : store.fallbackServer) : store.awayRoute)
            }
        } header: { Text(verbatim: "Ways to \(server)") }
    }

    /// Torrbaby's link in one line: the path it takes, the relay, and the last punch.
    private var linkDetail: String {
        var parts: [String] = []
        #if os(iOS)
        if let state = store.linkState {
            switch state.state {
            case "direct": parts.append("direct" + (state.rttMs.map { ", \($0) ms" } ?? ""))
            case "relay": parts.append("through bronze's relay" + (state.punching.map { "; punching: \($0)" } ?? ""))
            case "home": parts.append("straight to \(server) on this network")
            default: parts.append("no path answers now")
            }
            parts.append("relay " + (state.relay == "tcp" ? "over TCP 443 (this network blocks UDP)" : state.relay ?? "?"))
            if let punch = state.lastPunch { parts.append("last punch: \(punch.summary)") }
            if let address = state.publicIp, !address.isEmpty { parts.append("this network's public address \(address)") }
        } else {
            parts.append(store.linkRoute == nil ? "not running" : "starting")
        }
        #endif
        parts.append(store.relayAddress.isEmpty ? "bronze's address not known yet" : "bronze \(store.relayAddress)")
        if let connected = store.relayServerConnected { parts.append(connected ? "\(server) connected to bronze" : "\(server) NOT connected to bronze") }
        return parts.joined(separator: " · ")
    }

    private func row(_ title: String, kind: String, inUse: Bool, detail: String, showCheck: Bool = true) -> some View {
        let check = showCheck ? store.routeChecks[kind] : nil
        return VStack(alignment: .leading, spacing: 3) {
            HStack {
                Text(verbatim: title).font(.body.weight(inUse ? .semibold : .regular))
                if inUse { Text(verbatim: "in use").font(.caption.weight(.semibold)).padding(.horizontal, 6).padding(.vertical, 2).background(.green.opacity(0.2), in: Capsule()).foregroundStyle(.green) }
                Spacer()
                if let check {
                    Image(systemName: check.ok ? "checkmark.circle.fill" : "xmark.circle.fill").foregroundStyle(check.ok ? .green : .orange)
                }
            }
            if !detail.isEmpty { Text(verbatim: detail).font(.caption).foregroundStyle(.secondary) }
            if let check {
                Text(verbatim: "Last check \(clock(check.at)): \(check.detail)\(check.ms.map { " in \($0) ms" } ?? "")")
                    .font(.caption2.monospaced()).foregroundStyle(.secondary)
            }
        }.selectableText().padding(.vertical, 2)
    }

    #if os(iOS)
    /// Torrbaby's link (ABT's shared abt-link): what it is doing, its punches and its relay.
    @ViewBuilder private var directSection: some View {
        SwiftUI.Section {
            if let state = store.linkState {
                labeled("Path", state.state + (state.rttMs.map { ", \($0) ms round trip" } ?? "") + (state.lastAnsweredS.map { $0 > 10 ? ", last answered \($0) s ago" : "" } ?? ""))
                if let punching = state.punching { labeled("Punching now", punching) }
                if let punch = state.lastPunch {
                    labeled("Last punch", (punch.ok ? "opened: " : "failed: ") + punch.summary + (punch.at.map { " (\(clock(Date(timeIntervalSince1970: $0))))" } ?? ""))
                    if let next = punch.nextTry, next > Date().timeIntervalSince1970 { labeled("Next try", "in \(Int(next - Date().timeIntervalSince1970)) s") }
                }
                if let note = state.note, !note.isEmpty { labeled("Punch", note) }
                if let line = state.relayLine { labeled("Relay", line) }
                if state.behindDiamondsNat == true { labeled("Network", "same public address as \(server): relay only, no punching") }
                if let bytes = state.bytes {
                    let carried = ["home", "direct", "relay"].compactMap { key in bytes[key].flatMap { $0.received + $0.sent > 0 ? "\(key) ↓ \(Self.bytes($0.received)) ↑ \(Self.bytes($0.sent))" : nil } }
                    labeled("Carried", carried.isEmpty ? "nothing yet" : carried.joined(separator: " · "))
                }
                if let notes = state.notes, !notes.isEmpty {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(verbatim: "Link log").font(.caption).foregroundStyle(.secondary)
                        ForEach(Array(notes.suffix(12).enumerated()), id: \.offset) { Text(verbatim: $0.element).font(.caption2.monospaced()).foregroundStyle(.secondary) }
                    }.selectableText()
                }
                Button { TorrbabyLink.resumed() } label: { Text(verbatim: "Punch again now") }.disabled(!store.usingFallback)
            } else {
                labeled("State", store.linkRoute == nil ? (store.usingFallback ? "not running" : "not needed: on the home network") : "starting")
            }
        } header: { Text(verbatim: "Torrbaby link: details") }
    }

    /// Torrbaby's packet tunnel: for devices without Tailscale (iOS connects one VPN at a time,
    /// so turning this on disconnects Tailscale). It holds the direct path while the app is
    /// suspended, so background downloads keep going.
    @ViewBuilder private var tunnelSection: some View {
        SwiftUI.Section {
            labeled("State", tunnel.statusText)
            // The extension's report only while it runs: its last one would be stale.
            if tunnel.connected, let report = tunnelPaths { labeled("Paths", report) }
            // End-to-end encryption through bronze needs Diamond's identity, learned only where
            // bronze cannot interfere.
            labeled("Diamond's identity", store.diamondIdentity.isEmpty
                ? "not known yet: learned the next time this device reaches Diamond on the home network, or from pairing. Until then the relay is not encrypted end to end."
                : "\(store.diamondIdentity.prefix(16))…, learned \(store.diamondIdentitySource.isEmpty ? "at home" : store.diamondIdentitySource)")
            if let error = tunnel.lastError { labeled("Last error", error) }
            if !tunnel.connected, let why = tunnel.lastDisconnect { labeled("Why it last disconnected (iOS)", why) }
            // The extension's own log (App Group), newest last: what it did, including just
            // before it stopped.
            let lines = TunnelLog.read(15)
            if !lines.isEmpty {
                VStack(alignment: .leading, spacing: 2) {
                    Text(verbatim: "Tunnel log").font(.caption).foregroundStyle(.secondary)
                    ForEach(Array(lines.enumerated()), id: \.offset) { Text(verbatim: $0.element).font(.caption2.monospaced()).foregroundStyle(.secondary) }
                }.selectableText()
            } else if !TunnelLog.available {
                labeled("Tunnel log", "not available (shared keychain group missing)")
            }
            if tunnel.installed {
                if tunnel.connected {
                    Button { Task { _ = await tunnel.ask("punch") } } label: { Text(verbatim: "Punch the direct path again") }
                    Button { tunnel.disconnect() } label: { Text(verbatim: "Disconnect") }
                } else {
                    Button { Task { await store.connectTunnel() } } label: { Text(verbatim: "Connect") }
                }
                Button(role: .destructive) { Task { await tunnel.turnOff() } } label: { Text(verbatim: "Remove Torrbaby tunnel") }
            } else {
                Button { Task { await store.connectTunnel() } } label: { Text(verbatim: "Turn on Torrbaby tunnel") }
                .disabled(store.relayAddress.isEmpty || store.relayFingerprint.count != 64 || store.token.isEmpty)
            }
        } header: { Text(verbatim: "Torrbaby tunnel: details") }
    }

    @ViewBuilder private var eventsSection: some View {
        if !store.directEvents.isEmpty {
            SwiftUI.Section {
                ForEach(store.directEvents.reversed(), id: \.self) { Text(verbatim: $0).font(.caption2.monospaced()).foregroundStyle(.secondary).selectableText() }
            } header: { Text(verbatim: "Recent events") }
        }
    }
    #endif

    /// How reliably each carrier punches, from Diamond's record of every attempt (30 days).
    @ViewBuilder private var punchResultsSection: some View {
        if !punchResults.isEmpty {
            SwiftUI.Section {
                ForEach(punchResults, id: \.self) { r in
                    let total = r.punched + r.failed
                    let failures = (r.failedBySpread ?? [:]).sorted { $0.key < $1.key }.map { "\($0.value) \($0.key)" }.joined(separator: ", ")
                    labeled("\(r.carrier) · \(r.kind)", "\(r.punched) of \(total) punched" + (total > 0 ? " (\(Int((Double(r.punched) / Double(total) * 100).rounded()))%)" : "")
                        + (r.medianMs.map { ", median \($0) ms" } ?? "") + (failures.isEmpty ? "" : "; failed: \(failures)"))
                }
            } header: { Text(verbatim: "Punch results (last 30 days)") }
        }
    }
    /// Who uses bronze's relay and how much it carries (bronze's own count, China days).
    @ViewBuilder private var relayUsageSection: some View {
        if let usage = relayUsage {
            SwiftUI.Section {
                if usage.devices.isEmpty { labeled("Devices", "none on the relay now") }
                ForEach(usage.devices, id: \.self) { device in
                    let minutes = Int(max(0, Date().timeIntervalSince1970 - device.since) / 60)
                    labeled(device.name ?? (device.client.isEmpty ? "A device (an older build)" : "Device \(device.client.prefix(8))"),
                            "\(device.address), connected \(minutes < 60 ? "\(minutes) min" : "\(minutes / 60) h \(minutes % 60) min"), ↓ \(Self.bytes(device.down)) ↑ \(Self.bytes(device.up))")
                }
                let today = usage.days[usage.today]
                let month = usage.days.filter { $0.key.hasPrefix(String(usage.today.prefix(7))) }.values
                let recent = usage.days.keys.sorted().suffix(30).compactMap { usage.days[$0] }
                labeled("Today", today.map { "↓ \(Self.bytes($0.down)) ↑ \(Self.bytes($0.up))" } ?? "nothing yet")
                labeled("This month", "↓ \(Self.bytes(month.reduce(0) { $0 + $1.down })) ↑ \(Self.bytes(month.reduce(0) { $0 + $1.up }))")
                labeled("Last 30 days", "↓ \(Self.bytes(recent.reduce(0) { $0 + $1.down })) ↑ \(Self.bytes(recent.reduce(0) { $0 + $1.up }))")
            } header: { Text(verbatim: "Relay through bronze") } footer: { Text(verbatim: "Counted by bronze; days in China time.") }
        }
    }
    private static func bytes(_ count: Int64) -> String { ByteCountFormatter.string(fromByteCount: count, countStyle: .file) }
    private func loadRelayUsage() async {
        if let loaded: RelayUsage = try? await store.request("GET", "/v1/relay-usage", timeout: 15) { relayUsage = loaded }
    }
    private func loadPunchResults() async {
        if let loaded: PunchResults = try? await store.request("GET", "/v1/punch-results", timeout: 15) { punchResults = loaded.results }
    }

    private var actionsSection: some View {
        SwiftUI.Section {
            Button {
                measuring = true
                Task { await store.measurePaths(); await loadPunchResults(); measuring = false }
            } label: {
                HStack { Text(verbatim: measuring ? "Testing every way…" : "Test all ways now"); if measuring { Spacer(); ProgressView() } }
            }.disabled(measuring)
            Button { copy(report) } label: { Text(verbatim: "Copy report") }
            // The report, the tunnel's log, playback notes and the app's own log, saved on
            // Diamond under client-logs/: automatically (periodically and after failures) or now.
            Toggle(isOn: $automaticLogs) { Text("Send logs automatically") }
            Button {
                sendingLogs = true; logResult = nil
                Task {
                    do { logResult = L10n.text("Logs sent") + " · " + (try await ClientLogs.send(store: store, report: report)) }
                    catch { logResult = L10n.text("Couldn’t send logs") + " · " + error.localizedDescription }
                    sendingLogs = false
                }
            } label: {
                HStack { Text(LocalizedStringKey(sendingLogs ? "Sending logs…" : "Send logs now")); if sendingLogs { Spacer(); ProgressView() } }
            }.disabled(sendingLogs)
            if let logResult { Text(verbatim: logResult).font(.caption).foregroundStyle(.secondary).selectableText() }
        }
    }

    private func labeled(_ title: String, _ value: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(verbatim: title).font(.caption).foregroundStyle(.secondary)
            Text(verbatim: value).font(.callout)
        }.selectableText()
    }

    private var server: String { store.serverName.isEmpty ? "the server" : store.serverName }
    private func ms(_ kind: String) -> String { store.routeChecks[kind]?.ms.map { "\($0) ms" } ?? "" }
    private func clock(_ date: Date) -> String { date.formatted(date: .omitted, time: .standard) }
    private func symbol(_ path: Store.Path) -> String {
        switch path { case .home: "house.fill"; case .tunnel, .abt: "lock.shield.fill"; case .direct: "bolt.horizontal.fill"; case .relay: "arrow.triangle.branch"; case .tailscale: "network.badge.shield.half.filled"; case .offline: "wifi.slash" }
    }
    private func color(_ path: Store.Path) -> Color {
        switch path { case .home, .tunnel, .abt, .direct: .green; case .relay: .blue; case .tailscale: .orange; case .offline: .red }
    }

    /// Everything on the page as plain text, to paste into a message.
    private var report: String {
        var lines = ["Torrbaby connection report \(Date().formatted(date: .abbreviated, time: .standard))", AppRelease.description,
                     "Connected by: \(store.path.rawValue) — \(summary)", "Route: \(store.activeServer ?? "none")"]
        for (kind, title) in [("home", "Home network"), ("tunnel", "Torrbaby tunnel"), ("abt", "ABT"), ("direct", "Direct"), ("relay", "Relay"), ("tailscale", "Tailscale")] {
            if let check = store.routeChecks[kind] { lines.append("\(title): \(check.ok ? "ok" : "failed") at \(clock(check.at)), \(check.detail)\(check.ms.map { ", \($0) ms" } ?? "")") }
        }
        #if os(iOS)
        lines.append("Link: \(linkDetail)")
        lines += (store.linkState?.notes ?? []).map { "  link: \($0)" }
        lines.append(versions)
        lines.append("Torrbaby tunnel: \(tunnel.statusText)" + (tunnel.lastDisconnect.map { ", last disconnect: \($0)" } ?? ""))
        lines.append("Tunnel log:")
        lines += TunnelLog.read(60).map { "  \($0)" }
        lines.append("Events:")
        lines += store.directEvents.map { "  \($0)" }
        #endif
        return lines.joined(separator: "\n")
    }

    private func copy(_ text: String) {
        #if os(iOS)
        UIPasteboard.general.string = text
        #elseif os(macOS)
        NSPasteboard.general.clearContents(); NSPasteboard.general.setString(text, forType: .string)
        #endif
    }
}

/// How this device reaches the server, in a few words: "Connected via ABT, direct",
/// "Connected via relay", "Offline". The same line heads Settings → General and Settings →
/// Connection; the second adds the tunnel's source and way (ConnectionWay).
struct ConnectionWay {
    enum Source { case abt, torrbaby, none }
    enum Way { case home, direct, relay, tailscale }
    let source: Source
    let way: Way?
    let connecting: Bool

    @MainActor init(store: Store, tunnelState: String?) {
        guard store.connected else {
            way = nil
            // Before the first answer, or while ABT is finding its path after a network change.
            #if os(iOS)
            // With ABT on, the app waits for ABT to find its way to Diamond: ABT is the source.
            let abtBusy = ABT.connected
            #else
            let abtBusy = false
            #endif
            source = abtBusy ? .abt : .none
            connecting = !store.workOffline && store.networkAvailable && (store.connectionError == nil || abtBusy)
            return
        }
        connecting = false
        switch store.path {
        case .abt:
            source = .abt
            #if os(iOS)
            switch store.abtPath {
            case "lan": way = .home
            case "bronze": way = .relay
            default: way = .direct
            }
            #else
            way = .direct
            #endif
        case .tunnel:
            source = .torrbaby
            let state = tunnelState ?? ""
            way = state.contains("At home") ? .home : state.contains("Direct path: open") ? .direct : .relay
        case .direct: source = .torrbaby; way = .direct
        case .relay: source = .torrbaby; way = .relay
        case .home: source = .none; way = .home
        case .tailscale: source = .none; way = .tailscale
        case .offline: source = .none; way = nil
        }
    }

    var text: String {
        guard let way else { return connecting ? "Connecting…" : "Offline" }
        switch (source, way) {
        case (.abt, .home): return "Connected via ABT, home network"
        case (.abt, .direct): return "Connected via ABT, direct"
        case (.abt, _): return "Connected via ABT, relay"
        case (.torrbaby, .home): return "Connected via Torrbaby, home network"
        case (.torrbaby, .direct): return "Connected via direct"
        case (.torrbaby, _): return "Connected via relay"
        case (_, .home): return "Connected via home network"
        case (_, .direct): return "Connected via direct"
        case (_, .relay): return "Connected via relay"
        case (_, .tailscale): return "Connected via Tailscale"
        }
    }
    var icon: (String, Color) {
        guard let way else { return connecting ? ("arrow.triangle.2.circlepath", .orange) : ("wifi.slash", .red) }
        switch way {
        case .home: return ("house.fill", .green)
        case .direct: return ("bolt.horizontal.fill", .green)
        case .relay, .tailscale: return ("arrow.triangle.branch", .green)
        }
    }
    var sourceText: String {
        switch source { case .abt: "ABT"; case .torrbaby: "Torrbaby"; case .none: way == nil ? "None" : "None (no tunnel)" }
    }
    var wayText: String {
        switch way {
        case .home: "Home network"
        case .direct: "Direct"
        case .relay: "Relay"
        case .tailscale: "Tailscale"
        case nil: connecting ? "Connecting…" : "Offline"
        }
    }
}

/// The one-line status, with how long ago ABT's path last answered when that is a while.
struct ConnectionSummaryRow: View {
    @EnvironmentObject var store: Store
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    #if os(iOS)
    @ObservedObject private var tunnel = PacketTunnel.shared
    #endif
    @State private var tunnelState: String?
    @State private var tick = 0
    var body: some View {
        let _ = tick
        let way = ConnectionWay(store: store, tunnelState: tunnelState)
        HStack(spacing: 12) {
            Image(systemName: way.icon.0).font(.title3).foregroundStyle(way.icon.1).frame(width: 28)
            VStack(alignment: .leading, spacing: 2) {
                Text(LocalizedStringKey(way.text)).font(.headline)
                if let quiet = quietSeconds { Text(L10n.format("Last answered %lld s ago", quiet)).font(.caption).foregroundStyle(.secondary) }
            }
        }.padding(.vertical, 2)
        .task {
            while !Task.isCancelled {
                #if os(iOS)
                tunnelState = tunnel.connected ? await tunnel.ask() : nil
                #endif
                tick &+= 1
                try? await Task.sleep(for: .seconds(2))
            }
        }
    }
    /// Seconds since ABT's path last answered, when ABT carries the app and that is over 10 s.
    private var quietSeconds: Int? {
        #if os(iOS)
        guard store.path == .abt, let at = ABT.answeredAt else { return nil }
        let age = Int(Date().timeIntervalSince(at))
        return age > 10 ? age : nil
        #else
        return nil
        #endif
    }
}

/// Settings → Connection: the status, the tunnel's source and way, Work offline, logs, and the
/// last few connection events. The full technical account is one step further in.
struct ConnectionOverview: View {
    @EnvironmentObject var store: Store
    @Binding var showConnect: Bool
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    #if os(iOS)
    @ObservedObject private var tunnel = PacketTunnel.shared
    #endif
    @State private var tunnelState: String?
    @AppStorage(ClientLogs.automaticKey) private var automaticLogs = false
    @State private var sendingLogs = false
    @State private var logResult: String?
    var body: some View {
        let way = ConnectionWay(store: store, tunnelState: tunnelState)
        SwiftUI.Section { ConnectionSummaryRow() }
        SwiftUI.Section { Toggle("Work offline", isOn: $store.workOffline) }
        #if os(iOS)
        SwiftUI.Section {
            LabeledContent { Text(LocalizedStringKey(way.sourceText)) } label: { Text("Tunnel source") }
            LabeledContent { Text(LocalizedStringKey(way.wayText)) } label: { Text("Tunnel status") }
            if ABT.connected && store.abtIntercepted {
                Text("This Wi-Fi is intercepting connections. Sign in again.").foregroundStyle(.red)
            }
        }
        .task {
            while !Task.isCancelled {
                tunnelState = tunnel.connected ? await tunnel.ask() : nil
                try? await Task.sleep(for: .seconds(2))
            }
        }
        #endif
        SwiftUI.Section {
            Toggle("Send logs automatically", isOn: $automaticLogs)
            Button {
                sendingLogs = true; logResult = nil
                Task {
                    do { logResult = L10n.text("Logs sent") + " · " + (try await ClientLogs.send(store: store, report: ClientLogs.report(store))) }
                    catch { logResult = L10n.text("Couldn’t send logs") + " · " + error.localizedDescription }
                    sendingLogs = false
                }
            } label: {
                HStack { Text(LocalizedStringKey(sendingLogs ? "Sending logs…" : "Send logs now")); if sendingLogs { Spacer(); ProgressView() } }
            }.disabled(sendingLogs)
            if let logResult { Text(verbatim: logResult).font(.caption).foregroundStyle(.secondary).selectableText() }
        }
        #if os(iOS)
        if !store.directEvents.isEmpty {
            SwiftUI.Section("Recent events") {
                ForEach(store.directEvents.suffix(8).reversed(), id: \.self) { Text(verbatim: $0).font(.caption2.monospaced()).foregroundStyle(.secondary).selectableText() }
            }
        }
        #endif
        SwiftUI.Section {
            NavigationLink {
                Form { ConnectionStatusSections() }.formStyle(.grouped).canvasBackground().navigationTitle(L10n.text("Connection details"))
            } label: { Text("Connection details") }
            Button("Edit connection") { showConnect = true }
        }
    }
}
