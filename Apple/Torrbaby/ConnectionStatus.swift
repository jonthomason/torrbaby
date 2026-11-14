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
            #if os(iOS)
            tunnelSection
            directSection
            eventsSection
            #endif
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
            LabeledContent { Text(verbatim: relayReachable) } label: { Text(verbatim: "Relay") }
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
    private var relayReachable: String {
        if let connected = store.relayServerConnected {
            return (store.relayRoute != nil || tunnel.connected ? "reachable" : "closed") + (connected ? ", \(server) connected" : ", \(server) NOT connected")
        }
        return store.relayAddress.isEmpty ? "unknown" : "not checked yet"
    }
    #endif

    private var summary: String {
        switch store.path {
        case .home: return ms("home")
        case .tunnel: return ms("tunnel")
        case .direct:
            #if os(iOS)
            return ms("direct") + (store.directOpenedAt.map { " · since \(clock($0))" } ?? "")
            #else
            return ""
            #endif
        case .relay: return ms("relay")
        case .tailscale: return ms("tailscale")
        case .offline: return store.connectionError ?? ""
        }
    }

    @ViewBuilder private var pathSections: some View {
        SwiftUI.Section {
            row("Home network", kind: "home", inUse: store.path == .home, detail: store.homeRoute.isEmpty ? store.primaryAddress : store.homeRoute)
            #if os(iOS)
            row("Torrbaby tunnel", kind: "tunnel", inUse: store.path == .tunnel, detail: tunnel.installed ? tunnel.statusText + (tunnel.connected ? (tunnelPaths.map { "\n\($0)" } ?? "") : "") : "not installed")
            row("Direct (in the app)", kind: "direct", inUse: store.path == .direct, detail: store.directRoute == nil ? (store.directWhyNot ?? "") : "open" + (store.directOpenedAt.map { " since \(clock($0))" } ?? ""))
            row("Relay via bronze", kind: "relay", inUse: store.path == .relay, detail: relayDetail)
            #endif
            if store.tailscaleInUse {
                row("Tailscale", kind: "tailscale", inUse: store.path == .tailscale, detail: store.awayRoute.isEmpty ? (store.fallbackServer.isEmpty ? "not set up" : store.fallbackServer) : store.awayRoute)
            }
        } header: { Text(verbatim: "Ways to \(server)") }
    }

    private var relayDetail: String {
        var parts: [String] = []
        parts.append(store.relayAddress.isEmpty ? "bronze's address not known yet" : "bronze \(store.relayAddress)")
        parts.append(store.relayRoute == nil ? "relay closed" : "relay open")
        #if os(iOS)
        if let address = store.relayPublicAddress { parts.append("this network's public address \(address)") }
        #endif
        if let connected = store.relayServerConnected { parts.append(connected ? "\(server) connected to bronze" : "\(server) NOT connected to bronze") }
        return parts.joined(separator: " · ")
    }

    private func row(_ title: String, kind: String, inUse: Bool, detail: String) -> some View {
        let check = store.routeChecks[kind]
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
    @ViewBuilder private var directSection: some View {
        SwiftUI.Section {
            if store.directRoute != nil {
                labeled("State", "open" + (store.directOpenedAt.map { " since \(clock($0))" } ?? ""))
            } else {
                labeled("State", store.directWhyNot ?? "")
                if let next = store.nextDirectTry { labeled("Next try", "\(clock(next)) (in \(Int(next.timeIntervalSinceNow)) s)") }
            }
            if let attempt = store.directAttempt {
                labeled("Last attempt", "\(clock(attempt.started)) over \(attempt.signal)" + (attempt.finished.map { ", took \(String(format: "%.1f", $0.timeIntervalSince(attempt.started))) s" } ?? ", still running"))
                labeled("Step", attempt.ok ? "open" : attempt.failure == nil ? attempt.step : "failed while \(attempt.step)")
                if let failure = attempt.failure { labeled("Why", failure) }
                if let view = attempt.serverView { labeled("Diamond saw", view) }
                labeled("Network", attempt.interface.map { $0 == "en0" ? "Wi-Fi (en0)" : $0 == "pdp_ip0" ? "cellular (pdp_ip0)" : $0 } ?? "none")
                if let address = attempt.publicAddress { labeled("This network's public address", "\(address) (\(attempt.answered) of \(attempt.asked) sockets answered STUN)") }
                if let diagnosis = attempt.diagnosis { labeled("This network's NAT", diagnosis) }
                if let window = attempt.window { labeled("Ports Diamond sprays", window) }
                if let server = attempt.server { labeled("Diamond's public address", server) }
                if let ms = attempt.punchMs { labeled("Punch", "\(ms) ms") }
                if let ms = attempt.quicMs { labeled("QUIC handshake", "\(ms) ms") }
            }
            Button(store.directRoute == nil ? "Try direct now" : "Reconnect direct") {
                if store.directRoute != nil { store.dropDirect("reconnect requested") }
                store.tryDirectNow()
            }.disabled(!store.connected || !store.usingFallback)
        } header: { Text(verbatim: "Direct (in the app): details") }
    }

    /// Torrbaby's packet tunnel: for devices without Tailscale (iOS connects one VPN at a time,
    /// so turning this on disconnects Tailscale). It holds the direct path while the app is
    /// suspended, so background downloads keep going.
    @ViewBuilder private var tunnelSection: some View {
        SwiftUI.Section {
            labeled("State", tunnel.statusText)
            // The extension's report only while it runs: its last one would be stale.
            if tunnel.connected, let report = tunnelPaths { labeled("Paths", report) }
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
                    Button {
                        Task { await tunnel.turnOn(relay: store.relayAddress, relayFingerprint: store.relayFingerprint, stun: store.stunServer ?? "", token: store.token) }
                    } label: { Text(verbatim: "Connect") }
                }
                Button(role: .destructive) { Task { await tunnel.turnOff() } } label: { Text(verbatim: "Remove Torrbaby tunnel") }
            } else {
                Button {
                    Task { await tunnel.turnOn(relay: store.relayAddress, relayFingerprint: store.relayFingerprint, stun: store.stunServer ?? "", token: store.token) }
                } label: { Text(verbatim: "Turn on Torrbaby tunnel") }
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

    private var actionsSection: some View {
        SwiftUI.Section {
            Button {
                measuring = true
                Task { await store.measurePaths(); measuring = false }
            } label: {
                HStack { Text(verbatim: measuring ? "Testing every way…" : "Test all ways now"); if measuring { Spacer(); ProgressView() } }
            }.disabled(measuring)
            Button { copy(report) } label: { Text(verbatim: "Copy report") }
            #if os(iOS)
            NavigationLink { DirectConnectionTest() } label: { Text(verbatim: "Hole punch test (diagnostic)") }
            #endif
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
        switch path { case .home: "house.fill"; case .tunnel: "lock.shield.fill"; case .direct: "bolt.horizontal.fill"; case .relay: "arrow.triangle.branch"; case .tailscale: "network.badge.shield.half.filled"; case .offline: "wifi.slash" }
    }
    private func color(_ path: Store.Path) -> Color {
        switch path { case .home, .tunnel, .direct: .green; case .relay: .blue; case .tailscale: .orange; case .offline: .red }
    }

    /// Everything on the page as plain text, to paste into a message.
    private var report: String {
        var lines = ["Torrbaby connection report \(Date().formatted(date: .abbreviated, time: .standard))", AppRelease.description,
                     "Connected by: \(store.path.rawValue) — \(summary)", "Route: \(store.activeServer ?? "none")"]
        for (kind, title) in [("home", "Home network"), ("tunnel", "Torrbaby tunnel"), ("direct", "Direct"), ("relay", "Relay"), ("tailscale", "Tailscale")] {
            if let check = store.routeChecks[kind] { lines.append("\(title): \(check.ok ? "ok" : "failed") at \(clock(check.at)), \(check.detail)\(check.ms.map { ", \($0) ms" } ?? "")") }
        }
        #if os(iOS)
        lines.append("Relay: \(relayDetail)")
        lines.append("Direct: \(store.directRoute == nil ? (store.directWhyNot ?? "") : "open")")
        if let attempt = store.directAttempt {
            lines.append("Last direct attempt \(clock(attempt.started)) over \(attempt.signal): step \(attempt.step), \(attempt.failure ?? (attempt.ok ? "open" : "running"))")
            lines.append("  NAT: \(attempt.diagnosis ?? "?"), spray \(attempt.spread ?? "?")")
            lines.append("  network \(attempt.interface ?? "none"), public \(attempt.publicAddress ?? "?") (\(attempt.answered)/\(attempt.asked)), window \(attempt.window ?? "?"), Diamond \(attempt.server ?? "?"), punch \(attempt.punchMs.map(String.init) ?? "-") ms, QUIC \(attempt.quicMs.map(String.init) ?? "-") ms")
            if let view = attempt.serverView { lines.append("  Diamond saw: \(view)") }
        }
        lines.append(versions)
        lines.append(store.directDiagnostics)
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
