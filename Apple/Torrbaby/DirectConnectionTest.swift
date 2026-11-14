#if os(iOS)
import SwiftUI
import Darwin

/// Feasibility test for reaching the server directly instead of through Tailscale's relay
/// (the server half is server/punch.rs). The phone opens a batch of UDP sockets on its real
/// network (not the VPN, so an exit node does not matter), learns their public ports from a
/// STUN server, and reports them over the normal connection. The server sprays the likely
/// window of the phone's next ports while the phone's sockets fire at the server's public
/// address. Any packet that gets through proves a direct path. Nothing else uses it.
struct DirectConnectionTest: View {
    @EnvironmentObject var store: Store
    @State private var running = false
    @State private var lines: [String] = []
    @State private var outcome: Bool?

    var body: some View {
        List {
            SwiftUI.Section {
                Button { Task { await run() } } label: {
                    HStack {
                        Text(running ? "Testing direct connection…" : "Run test")
                        if running { Spacer(); ProgressView() }
                    }
                }.disabled(running || !store.connected)
                if let outcome {
                    Label(outcome ? "Direct connection works" : "Direct connection failed",
                          systemImage: outcome ? "checkmark.circle.fill" : "xmark.circle.fill")
                        .foregroundStyle(outcome ? .green : .orange)
                }
            } footer: {
                Text("Checks whether this device can reach your server directly instead of through Tailscale’s relay. Nothing else uses it.")
            }
            SwiftUI.Section("Direct route") {
                Text(verbatim: AppRelease.description).font(.caption.monospaced()).foregroundStyle(.secondary)
                Text(verbatim: store.directRoute.map { "In use: \($0) (\(store.directNote))" } ?? "Not in use\(store.directNote.isEmpty ? "" : " (\(store.directNote))")")
                    .font(.caption.monospaced())
                TimelineView(.periodic(from: .now, by: 1)) { _ in
                    Text(verbatim: store.directDiagnostics).font(.caption2.monospaced()).foregroundStyle(.secondary)
                }
                Button("Try direct now") { store.tryDirectNow() }.disabled(store.directRoute != nil)
                ForEach(store.directEvents.reversed(), id: \.self) { Text(verbatim: $0).font(.caption2.monospaced()).foregroundStyle(.secondary) }
            }
            if !lines.isEmpty {
                SwiftUI.Section("Details") {
                    ForEach(lines.indices, id: \.self) { Text(verbatim: lines[$0]).font(.caption.monospaced()).textSelection(.enabled) }
                }
            }
        }
        .navigationTitle(L10n.text("Direct connection test"))
    }

    private struct Started: Decodable { let id: String; let server: String; let low: Int; let high: Int; let seconds: Int }
    private struct Report: Decodable {
        let state: String; let connectedFrom: String?; let connectedAfterMs: Int?; let sprays: Int?; let received: Int?
    }

    private func run() async {
        running = true; outcome = nil; lines = []
        defer { running = false }
        let probe = PunchProbe(count: 128)
        lines.append("Network: \(probe.interface ?? "default")")
        guard probe.ready else { lines.append("Could not open sockets"); outcome = false; return }
        guard let stun = store.stunServer else { lines.append("STUN: the rendezvous is not known yet"); outcome = false; probe.close(); return }
        let mappings = await Task.detached { probe.discover(stun: stun, limit: 12) }.value
        guard let ip = mappings.first?.ip else { lines.append("STUN: no answer"); outcome = false; probe.close(); return }
        let ports = mappings.map(\.port).sorted()
        let steps = zip(ports, ports.dropFirst()).map { $1 - $0 }
        lines.append("Public address (STUN): \(ip), \(mappings.count) of \(probe.count) sockets answered")
        lines.append("Ports \(ports.first!)–\(ports.last!), typical step \(steps.sorted().dropFirst(steps.count / 2).first ?? 0)")
        let span = ports.last! - ports.first!
        let low = max(1, ports.first!), high = min(65535, ports.last! + span + 512)
        do {
            let started: Started = try await store.request("POST", "/v1/punch", body: ["ip": ip, "low": low, "high": high], timeout: 30)
            lines.append("Diamond: \(started.server), spraying \(started.low)–\(started.high)")
            let result = await Task.detached { probe.fire(at: started.server, id: started.id, seconds: 20) }.value
            if let spray = result.firstSpray { lines.append("First spray arrived after \(spray.afterMs) ms on socket \(spray.socket)") }
            if let hit = result.hit {
                lines.append("Both ways after \(hit.afterMs) ms on socket \(hit.socket)")
                if !result.roundTrips.isEmpty {
                    let sorted = result.roundTrips.sorted()
                    lines.append("Round trip: median \(sorted[sorted.count / 2]) ms, best \(sorted.first!) ms (\(sorted.count) pings)")
                }
            } else {
                lines.append("Nothing got through in 20 s")
            }
            if let report: Report = try? await store.request("GET", "/v1/punch/\(started.id)", timeout: 15) {
                lines.append("Diamond saw: \(report.state), heard from \(report.connectedFrom ?? "nobody")"
                    + (report.connectedAfterMs.map { " after \($0) ms" } ?? "") + ", \(report.received ?? 0) packets in, \(report.sprays ?? 0) sprays")
            }
            outcome = result.hit != nil
        } catch {
            lines.append("Error: \(error.localizedDescription)")
            outcome = false
        }
        probe.close()
    }
}

#endif
