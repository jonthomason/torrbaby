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
            #if os(iOS)
            directSection
            eventsSection
            #endif
            actionsSection
        }
        .task {
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(1))
                tick &+= 1
            }
        }
    }

    private var nowSection: some View {
        SwiftUI.Section {
            HStack(spacing: 12) {
                Image(systemName: symbol(store.path)).font(.title2).foregroundStyle(color(store.path)).frame(width: 32)
                VStack(alignment: .leading, spacing: 2) {
                    Text(verbatim: store.path.rawValue).font(.title3.weight(.semibold))
                    Text(verbatim: summary).font(.subheadline).foregroundStyle(.secondary)
                }
            }.padding(.vertical, 4)
        } header: { Text(verbatim: "Connected by") }
    }

    private var summary: String {
        switch store.path {
        case .home: return "On the same network as \(server)" + ms("home")
        case .direct:
            #if os(iOS)
            return "Straight to \(server) through both NATs, no server in between" + ms("direct") + (store.directOpenedAt.map { ", since \(clock($0))" } ?? "")
            #else
            return ""
            #endif
        case .relay: return "Through bronze (\(store.relayAddress)), which passes requests to \(server)" + ms("relay")
        case .tailscale: return "Through Tailscale" + ms("tailscale")
        case .offline: return store.connectionError ?? "No way to \(server) answered"
        }
    }

    @ViewBuilder private var pathSections: some View {
        SwiftUI.Section {
            row("Home network", kind: "home", inUse: store.path == .home, detail: store.homeRoute.isEmpty ? store.primaryAddress : store.homeRoute)
            #if os(iOS)
            row("Direct (hole punch)", kind: "direct", inUse: store.path == .direct, detail: store.directRoute == nil ? (store.directWhyNot ?? "") : "open, QUIC on the punched socket")
            row("Relay via bronze", kind: "relay", inUse: store.path == .relay, detail: relayDetail)
            #endif
            row("Tailscale", kind: "tailscale", inUse: store.path == .tailscale, detail: !store.tailscaleInUse ? "not used: the relay through bronze replaces it" : store.awayRoute.isEmpty ? (store.fallbackServer.isEmpty ? "not set up" : store.fallbackServer) : "\(store.awayRoute) (to be retired)")
        } header: { Text(verbatim: "Ways to \(server)") } footer: {
            Text(verbatim: store.tailscaleInUse ? "Preference: home network, then direct, then the relay, then Tailscale. Each line shows its last check." : "Preference: home network, then direct, then the relay. Each line shows its last check.")
        }
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
        } header: { Text(verbatim: "Direct connection") } footer: {
            Text(verbatim: "Away from home, the app punches through this network's NAT and China Telecom's at the same moment, so traffic goes straight to \(server). It is tried as soon as the online check succeeds, after a drop, and every 3 minutes after a failure.")
        }
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
    private func ms(_ kind: String) -> String { store.routeChecks[kind]?.ms.map { ", \($0) ms round trip" } ?? "" }
    private func clock(_ date: Date) -> String { date.formatted(date: .omitted, time: .standard) }
    private func symbol(_ path: Store.Path) -> String {
        switch path { case .home: "house.fill"; case .direct: "bolt.horizontal.fill"; case .relay: "arrow.triangle.branch"; case .tailscale: "network.badge.shield.half.filled"; case .offline: "wifi.slash" }
    }
    private func color(_ path: Store.Path) -> Color {
        switch path { case .home, .direct: .green; case .relay: .blue; case .tailscale: .orange; case .offline: .red }
    }

    /// Everything on the page as plain text, to paste into a message.
    private var report: String {
        var lines = ["Torrbaby connection report \(Date().formatted(date: .abbreviated, time: .standard))", AppRelease.description,
                     "Connected by: \(store.path.rawValue) — \(summary)", "Route: \(store.activeServer ?? "none")"]
        for (kind, title) in [("home", "Home network"), ("direct", "Direct"), ("relay", "Relay"), ("tailscale", "Tailscale")] {
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
        lines.append(store.directDiagnostics)
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
