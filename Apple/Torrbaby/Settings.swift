import SwiftUI
import UniformTypeIdentifiers
#if os(macOS)
import AppKit
#endif

struct AutomationView: View {
    @EnvironmentObject var store: Store
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 24) {
                Text("A quieter kind of housekeeping.").font(.system(size: 30, weight: .bold, design: .rounded))
                Text("Complete downloads find their home. Seeding carries on.").foregroundStyle(Theme.muted)
                if let status = store.status {
                    Card {
                        VStack(alignment: .leading, spacing: 18) {
                            HStack {
                                Image(systemName: status.settings.paused ? "pause.circle.fill" : "sparkles").font(.title).foregroundStyle(Theme.mint)
                                VStack(alignment: .leading, spacing: 5) {
                                    Text(status.settings.paused ? "Taking a breather" : "Your organizer is running").font(.headline)
                                    Text("Checks every \(Int(status.settings.pollSeconds)) seconds").font(.caption).foregroundStyle(Theme.muted)
                                }
                                Spacer()
                                Button(status.settings.paused ? "Resume" : "Pause") { store.toggleAutomation() }.buttonStyle(.bordered)
                            }
                            if let error = status.lastError { Text(error).foregroundStyle(.orange) }
                            if let error = status.scanError { Label("Plex: " + error, systemImage: "exclamationmark.triangle").foregroundStyle(.orange) }
                            HStack {
                                Text("\(status.jobs.values.filter { $0.phase == "done" }.count) organized")
                                Text("·")
                                Text("\(status.pendingScans) pending scans")
                            }.font(.caption).foregroundStyle(Theme.muted)
                            HStack {
                                Button("Check now") { store.perform { try await store.send("POST", "/v1/reconcile") } }
                                Button("Scan Plex libraries") { store.perform { try await store.send("POST", "/v1/plex/scan") } }
                            }.buttonStyle(.bordered).disabled(store.busy)
                        }
                    }
                    let issues = status.jobs.values.filter { $0.error != nil || $0.phase == "moving" || $0.phase == "blocked" }.sorted { $0.name < $1.name }
                    if !issues.isEmpty {
                        Text("IN PROGRESS & NEEDS REVIEW").font(.caption.weight(.semibold)).tracking(1.4).foregroundStyle(Theme.muted)
                        ForEach(issues) { job in
                            NavigationLink(value: job.hash) {
                                Card {
                                    VStack(alignment: .leading, spacing: 8) {
                                        Text(job.name).font(.headline)
                                        Text(job.error ?? job.phase.capitalized).font(.caption).foregroundStyle(.orange)
                                    }
                                }
                            }.buttonStyle(.plain)
                        }
                    }
                }
                Text("RECENT ACTIVITY").font(.caption.weight(.semibold)).tracking(1.4).foregroundStyle(Theme.muted)
                if store.events.isEmpty {
                    Card { Text("All quiet. Completed downloads and organizer actions will appear here.").foregroundStyle(Theme.muted) }
                }
                LazyVStack(spacing: 0) {
                    ForEach(store.events.prefix(100)) { event in
                        HStack(alignment: .top, spacing: 15) {
                            Image(systemName: event.error == nil ? "checkmark.circle" : "exclamationmark.circle")
                                .foregroundStyle(event.error == nil ? Theme.mint : .orange).padding(.top, 2)
                            VStack(alignment: .leading, spacing: 5) {
                                Text(event.title).font(.callout.weight(.medium))
                                if let hash = event.hash, let torrent = store.torrents.first(where: { $0.id == hash }) {
                                    Text(torrent.displayName).font(.caption).foregroundStyle(Theme.muted).lineLimit(2)
                                }
                                if let error = event.error { Text(error).font(.caption).foregroundStyle(.orange) }
                                if let destination = event.destination { Text(destination).font(.caption2).foregroundStyle(Theme.muted) }
                            }
                            Spacer()
                            Text(Date(timeIntervalSince1970: event.time), style: .relative).font(.caption2).foregroundStyle(Theme.muted)
                        }.padding(.vertical, 14)
                        Divider().opacity(0.4)
                    }
                }
            }.padding(24).frame(maxWidth: 1000).frame(maxWidth: .infinity)
        }.background(Theme.canvas)
    }
}

struct SettingsView: View {
    @EnvironmentObject var store: Store
    @Binding var showConnect: Bool
    @State private var source = ""
    @State private var movies = ""
    @State private var tv = ""
    @State private var interval = 30.0
    @State private var saved = false
    @State private var magnetStatus: String?
    var body: some View {
        Form {
            SwiftUI.Section("Connection") {
                LabeledContent("Server", value: store.server.isEmpty ? "Not configured" : store.server)
                if let status = store.status { LabeledContent("Daemon", value: "Torrbaby \(status.version)") }
                Button("Edit connection") { showConnect = true }
            }
            SwiftUI.Section("Devices") {
                ForEach(store.clients) { client in
                    DeviceNameRow(client: client, isCurrent: client.id == store.clientId)
                }
                Text("Playback progress is shared across your devices. Each installation has its own stable ID.").font(.caption).foregroundStyle(.secondary)
            }
            SwiftUI.Section("Automation") {
                if let status = store.status {
                    Toggle("Organize completed downloads", isOn: Binding(get: { !status.settings.paused }, set: { _ in store.toggleAutomation() })).disabled(store.busy)
                }
                Stepper("Check every \(Int(interval)) seconds", value: $interval, in: 5...3600, step: 5)
                Text("Complete TV downloads are identified by season and episode markers. Other video downloads go to Movies.").font(.caption).foregroundStyle(.secondary)
            }
            SwiftUI.Section("Folders on your server") {
                TextField("Downloads", text: $source)
                TextField("Movies", text: $movies)
                TextField("TV shows", text: $tv)
                Text("These folders must already exist on your server and match your Plex library roots. Changes apply to future jobs.").font(.caption).foregroundStyle(.secondary)
                Button(saved ? "Saved" : "Save settings") {
                    store.perform {
                        try await store.send("PATCH", "/v1/settings", body: ["source": source, "movies": movies, "tv": tv, "poll_seconds": interval])
                        saved = true
                    }
                }.disabled(store.busy || !store.connected)
            }
            SwiftUI.Section("Playback prototype") {
                Label("Apple’s native player", systemImage: "play.rectangle")
                Text("Original files stream directly from your server. Supported formats use Apple’s playback pipeline. Unsupported formats fail without conversion. No transcoding or custom playback controls.")
                    .font(.callout).foregroundStyle(.secondary)
            }
            #if os(macOS)
            SwiftUI.Section("Magnet links") {
                Button("Use Torrbaby for magnet links") {
                    NSWorkspace.shared.setDefaultApplication(at: Bundle.main.bundleURL, toOpenURLsWithScheme: "magnet") { error in
                        Task { @MainActor in
                            magnetStatus = error?.localizedDescription ?? "Torrbaby is now your default magnet-link app."
                        }
                    }
                }
                Text(magnetStatus ?? "Open links from your browser in Torrbaby, review the download, then send it to your server.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            #endif
            SwiftUI.Section("About") {
                Text("Torrbaby 0.2 · MIT License")
                Text("Made for a small, well-kept personal library.").font(.caption).foregroundStyle(.secondary)
            }
        }.formStyle(.grouped)
        .onAppear {
            if let s = store.status?.settings {
                source = s.source; movies = s.movies; tv = s.tv; interval = s.pollSeconds
            }
        }
        .onChange(of: source) { _, _ in saved = false }
        .onChange(of: movies) { _, _ in saved = false }
        .onChange(of: tv) { _, _ in saved = false }
        .onChange(of: interval) { _, _ in saved = false }
    }
}

struct ConnectionView: View {
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) var dismiss
    @State private var address = ""
    @State private var token = ""
    @State private var importProfile = false
    var body: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 24) {
                Image(systemName: "arrow.down.forward.and.arrow.up.backward.circle.fill").font(.system(size: 55)).foregroundStyle(Theme.mint)
                VStack(alignment: .leading, spacing: 9) {
                    Text("Meet your server.").font(.system(size: 30, weight: .bold, design: .rounded))
                    Text("Connect to Torrbaby on your private network. Your downloads stay right where they belong.").foregroundStyle(Theme.muted)
                }
                VStack(alignment: .leading, spacing: 14) {
                    Text("SERVER ADDRESS").font(.caption2.weight(.semibold)).tracking(1.5).foregroundStyle(Theme.muted)
                    TextField("http://your-server:8765", text: $address).textFieldStyle(.roundedBorder)
                        #if os(iOS)
                        .textInputAutocapitalization(.never).autocorrectionDisabled().keyboardType(.URL)
                        #endif
                    Text("API TOKEN").font(.caption2.weight(.semibold)).tracking(1.5).foregroundStyle(Theme.muted)
                    SecureField("Paste your server token", text: $token).textFieldStyle(.roundedBorder)
                        #if os(iOS)
                        .textInputAutocapitalization(.never).autocorrectionDisabled()
                        #endif
                    Text("The token is saved in Keychain on this device. Use Tailscale or HTTPS when connecting remotely.").font(.caption).foregroundStyle(Theme.muted)
                    Button("Import connection file…") { importProfile = true }.font(.caption)
                }
                if let error = store.connectionError { Text(error).font(.caption).foregroundStyle(.orange) }
                Button {
                    store.server = address; store.token = token
                    Task { await store.connect(); if store.connected { dismiss() } }
                } label: {
                    HStack { Spacer(); if store.isConnecting { ProgressView().controlSize(.small) }; Text(store.isConnecting ? "Connecting…" : "Connect to server").fontWeight(.semibold); Spacer() }.padding(.vertical, 7)
                }.buttonStyle(.borderedProminent).foregroundStyle(Theme.canvas).disabled(address.isEmpty || token.isEmpty || store.isConnecting)
                Spacer(minLength: 0)
            }.padding(30).background(Theme.canvas)
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Close") { dismiss() } } }
            .onAppear { address = store.server; token = store.token }
            .fileImporter(isPresented: $importProfile, allowedContentTypes: [.json]) { result in
                do {
                    let url = try result.get()
                    let accessed = url.startAccessingSecurityScopedResource()
                    defer { if accessed { url.stopAccessingSecurityScopedResource() } }
                    let profile = try JSONDecoder().decode([String: String].self, from: Data(contentsOf: url))
                    guard let server = profile["server"], let secret = profile["token"] else {
                        throw APIError(message: "The file must contain server and token fields.")
                    }
                    address = server; token = secret
                } catch { store.connectionError = error.localizedDescription }
            }
        }
        #if os(macOS)
        .frame(width: 520, height: 580)
        #endif
    }
}

struct AddTorrentView: View {
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) var dismiss
    @State private var magnet = ""
    @State private var paused = false
    init(initialMagnet: String = "") { _magnet = State(initialValue: initialMagnet) }
    var body: some View {
        NavigationStack {
            Form {
                SwiftUI.Section("Magnet link") {
                    TextEditor(text: $magnet).frame(minHeight: 120).font(.body.monospaced())
                        #if os(iOS)
                        .textInputAutocapitalization(.never).autocorrectionDisabled()
                        #endif
                    Toggle("Add paused", isOn: $paused)
                }
                SwiftUI.Section {
                    Text("Downloads start in the configured Downloads folder. Torrbaby will organize them when every file is complete.").font(.callout).foregroundStyle(.secondary)
                }
            }.formStyle(.grouped).navigationTitle("Add a torrent")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Add") {
                        store.perform {
                            try await store.send("POST", "/v1/torrents", body: ["magnet": magnet.trimmingCharacters(in: .whitespacesAndNewlines), "paused": paused])
                            dismiss()
                        }
                    }.disabled(!magnet.trimmingCharacters(in: .whitespacesAndNewlines).hasPrefix("magnet:?") || store.busy || !store.connected)
                }
            }
        }
        #if os(macOS)
        .frame(width: 540, height: 390)
        #endif
    }
}

struct DeviceNameRow: View {
    @EnvironmentObject var store: Store
    let client: ClientDevice
    let isCurrent: Bool
    @State private var name = ""
    var body: some View {
        HStack {
            TextField(client.platform, text: $name)
            if isCurrent { Text("This device").font(.caption).foregroundStyle(.secondary) }
            Button("Save") {
                store.perform { try await store.send("PATCH", "/v1/clients/" + client.id, body: ["name": name]) }
            }.disabled(name.trimmingCharacters(in: .whitespaces).isEmpty || name == client.name || store.busy)
        }.onAppear { name = client.name }
        .onChange(of: client.name) { _, value in name = value }
    }
}
