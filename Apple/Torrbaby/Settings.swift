import SwiftUI
#if !os(tvOS)
import UniformTypeIdentifiers
#if os(iOS)
import UserNotifications
#endif
#if os(macOS)
import AppKit
#endif

struct AutomationView: View {
    @EnvironmentObject var store: Store
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 24) {
                if let status = store.status {
                    Card {
                        VStack(alignment: .leading, spacing: 18) {
                            HStack {
                                Image(systemName: status.settings.paused ? "pause.circle.fill" : "sparkles").font(.title).foregroundStyle(Theme.mint)
                                VStack(alignment: .leading, spacing: 5) {
                                    Text(status.settings.paused ? "Paused" : "Running").font(.headline)
                                    Text("Checks every \(Int(status.settings.pollSeconds)) seconds").font(.caption).foregroundStyle(Theme.muted)
                                }
                                Spacer()
                                Button(status.settings.paused ? "Resume" : "Pause") { store.toggleAutomation() }.buttonStyle(.bordered)
                            }
                            if let error = status.lastError { Text(error).foregroundStyle(.orange) }
                            if let error = status.scanError { Label("Library: " + error, systemImage: "exclamationmark.triangle").foregroundStyle(.orange) }
                            HStack {
                                Text("\(status.jobs.values.filter { $0.phase == "done" }.count) organized")
                                Text("·")
                                Text("\(status.pendingScans) pending scans")
                            }.font(.caption).foregroundStyle(Theme.muted)
                            HStack {
                                Button("Check now") { store.perform { try await store.send("POST", "/v1/reconcile") } }
                                Button("Scan media library") { store.perform { try await store.send("POST", "/v1/library/scan") } }
                            }.buttonStyle(.bordered).disabled(store.busy)
                        }
                    }
                    let issues = status.jobs.values.filter { $0.error != nil || $0.phase == "moving" || $0.phase == "blocked" }.sorted { $0.name < $1.name }
                    if !issues.isEmpty {
                        Text("Needs attention").font(.caption.weight(.semibold)).tracking(1.4).foregroundStyle(Theme.muted)
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
                Text("Recent activity").font(.caption.weight(.semibold)).tracking(1.4).foregroundStyle(Theme.muted)
                if store.events.isEmpty {
                    Card { Text("No recent activity").foregroundStyle(Theme.muted) }
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
    @AppStorage("activityAlerts") private var activityAlerts = true
    @State private var notificationStatus: String?
    #if os(macOS)
    @AppStorage("showMenuBarRates") private var showMenuBarRates = true
    #endif
    @State private var magnetStatus: String?
    @State private var showPairing = false
    @State private var showSubtitles = false
    var body: some View {
        Form {
            SwiftUI.Section("Torrbaby") {
                Text(AppRelease.description).monospacedDigit()
            }
            SwiftUI.Section("Playback") {
                Button("Default subtitles") { showSubtitles = true }
                    .sheet(isPresented: $showSubtitles) { SubtitlePreferencesView() }
            }
            SwiftUI.Section("Translations") {
                BackgroundSubtitlesView()
            }
            SwiftUI.Section("Connection") {
                LabeledContent("Server", value: store.server.isEmpty ? "Not configured" : store.server)
                if let status = store.status { LabeledContent("Daemon", value: "Torrbaby \(status.version)") }
                Button("Edit connection") { showConnect = true }
            }
            #if os(macOS)
            SwiftUI.Section("Menu bar") {
                Toggle("Show download and upload rates", isOn: $showMenuBarRates)
            }
            #endif
            SwiftUI.Section("Notifications") {
                Toggle("Download and seeding updates", isOn: $activityAlerts)
                #if os(iOS)
                Button("Enable device notifications") {
                    Task {
                        do {
                            let allowed = try await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound])
                            notificationStatus = allowed ? "Notifications enabled" : "Allow notifications in iOS Settings."
                        } catch { notificationStatus = error.localizedDescription }
                    }
                }
                if let notificationStatus { Text(notificationStatus).font(.caption) }
                #endif
            }
            SwiftUI.Section("Devices") {
                Button { showPairing = true } label: { Label("Link a device", systemImage: "qrcode") }
                    .disabled(!store.connected)
                    .sheet(isPresented: $showPairing) { PairingCodeView() }
                ForEach(store.clients) { client in
                    HStack {
                        DeviceNameRow(client: client, isCurrent: client.id == store.clientId)
                        if client.id != store.clientId && client.pairedCredential == 1 {
                            Button("Revoke access", role: .destructive) {
                                store.perform { try await store.send("DELETE", "/v1/device-credentials/" + client.id) }
                            }.font(.caption)
                        }
                    }
                }
            }
            SeedingSettings()
            SwiftUI.Section("Automation") {
                if let status = store.status {
                    Toggle("Organize completed downloads", isOn: Binding(get: { !status.settings.paused }, set: { _ in store.toggleAutomation() })).disabled(store.busy)
                }
                Stepper("Check every \(Int(interval)) seconds", value: $interval, in: 5...3600, step: 5)
            }
            SwiftUI.Section("Folders on your server") {
                TextField("Downloads", text: $source)
                TextField("Movies", text: $movies)
                TextField("TV shows", text: $tv)
                Button(saved ? "Saved" : "Save settings") {
                    store.perform {
                        try await store.send("PATCH", "/v1/settings", body: ["source": source, "movies": movies, "tv": tv, "poll_seconds": interval])
                        saved = true
                    }
                }.disabled(store.busy || !store.connected)
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
                if let magnetStatus { Text(magnetStatus).font(.caption).foregroundStyle(.secondary) }
            }
            #endif
            SwiftUI.Section("About") {
                Text("MIT License")
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

struct SeedingSettings: View {
    @EnvironmentObject var store: Store
    @State private var available = false
    @State private var enabled = true
    @State private var ratio = 1.2
    @State private var saved = false
    struct Policy: Decodable { let enabled: Bool; let ratio: Double }
    var body: some View {
        Group {
            if available {
                SwiftUI.Section("Seeding") {
                    Toggle("Move to Complete at target ratio", isOn: $enabled)
                    TextField("Target upload/download ratio", value: $ratio, format: .number.precision(.fractionLength(1...2)))
                        .disabled(!enabled)
                    Text("Individual torrent limits take precedence.")
                        .font(.caption).foregroundStyle(.secondary)
                    Button(saved ? "Saved" : "Save seeding settings") {
                        store.perform {
                            let policy: Policy = try await store.request("PATCH", "/v1/seeding", body: ["enabled":enabled, "ratio":ratio])
                            enabled = policy.enabled; ratio = policy.ratio; saved = true
                        }
                    }.disabled(store.busy || !store.connected || !ratio.isFinite || ratio <= 0 || ratio > 1000)
                }
            }
        }
        .task(id: store.server) {
            if let policy: Policy = try? await store.request("GET", "/v1/seeding") {
                enabled = policy.enabled; ratio = policy.ratio; available = true
            }
        }
        .onChange(of: enabled) { _, _ in saved = false }
        .onChange(of: ratio) { _, _ in saved = false }
    }
}

struct ConnectionView: View {
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) var dismiss
    @State private var address = ""
    @State private var token = ""
    @State private var importProfile = false
    #if os(iOS)
    @State private var showScanner = false
    #endif
    var body: some View {
        NavigationStack {
            #if os(iOS)
            VStack(spacing: 24) {
                Spacer()
                Image("TorrbabyLogo").resizable().frame(width: 80, height: 80)
                    .clipShape(RoundedRectangle(cornerRadius: 18)).accessibilityHidden(true)
                HStack(spacing: 20) {
                    ZStack {
                        Image(systemName: "desktopcomputer").font(.system(size: 76, weight: .light))
                        Image(systemName: "qrcode").font(.system(size: 27)).offset(y: -8)
                    }
                    Image(systemName: "arrow.right").font(.title3).foregroundStyle(Theme.muted)
                    Image(systemName: "iphone.gen3.radiowaves.left.and.right")
                        .font(.system(size: 58, weight: .light))
                }.foregroundStyle(Theme.mint).padding(.vertical, 12).accessibilityHidden(true)
                VStack(spacing: 8) {
                    Text("Link a device").font(.system(size: 28, weight: .bold, design: .rounded))
                    Text("On your connected device").font(.subheadline).foregroundStyle(Theme.muted)
                    Label("Settings → Link a device", systemImage: "gearshape").font(.subheadline)
                }.multilineTextAlignment(.center)
                Button { showScanner = true } label: {
                    Label("Scan connection code", systemImage: "qrcode.viewfinder")
                        .font(.headline).padding(.horizontal, 12).padding(.vertical, 10)
                }
                .buttonStyle(.borderedProminent).foregroundStyle(Theme.canvas)
                .sheet(isPresented: $showScanner) { PairingScannerView { dismiss() } }
                Spacer()
                Spacer().frame(height: 40)
            }.padding(30).frame(maxWidth: .infinity, maxHeight: .infinity)
                .background(Theme.canvas)
                .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Close") { dismiss() } } }
            #else
            VStack(alignment: .leading, spacing: 24) {
                Image(systemName: "arrow.down.forward.and.arrow.up.backward.circle.fill").font(.system(size: 55)).foregroundStyle(Theme.mint)
                VStack(alignment: .leading, spacing: 9) {
                    Text("Connect to Torrbaby").font(.system(size: 30, weight: .bold, design: .rounded))
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
            #endif
        }
        #if os(macOS)
        .frame(width: 520, height: 580)
        #endif
    }
}

// Preserve the encoded query (including every tracker); normalize browser variants
// such as magnet:/? and magnet://? to Transmission's canonical magnet:? form.
func canonicalMagnet(_ input: String) -> String {
    let trimmed = input.trimmingCharacters(in: .whitespacesAndNewlines)
    guard let colon = trimmed.firstIndex(of: ":"),
          trimmed[..<colon].lowercased() == "magnet",
          let query = trimmed[colon...].firstIndex(of: "?") else { return trimmed }
    let between = trimmed[trimmed.index(after: colon)..<query]
    guard between.allSatisfy({ $0 == "/" }) else { return trimmed }
    return "magnet:" + trimmed[query...]
}

struct AddTorrentView: View {
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) var dismiss
    @State private var magnet = ""
    @State private var paused = false
    @State private var submitting = false
    @State private var showProgress = false
    init(initialMagnet: String = "") { _magnet = State(initialValue: canonicalMagnet(initialMagnet)) }
    var body: some View {
        NavigationStack {
            Form {
                SwiftUI.Section("Magnet link") {
                    TextEditor(text: $magnet).frame(minHeight: 120).font(.body.monospaced())
                        #if os(iOS)
                        .textInputAutocapitalization(.never).autocorrectionDisabled()
                        #endif
                    Toggle("Add paused", isOn: $paused)
                }.disabled(submitting)
                if showProgress {
                    SwiftUI.Section {
                        HStack(spacing: 10) {
                            ProgressView().controlSize(.small)
                            Text("Adding download…").foregroundStyle(.secondary)
                        }
                        .accessibilityElement(children: .combine)
                        .accessibilityLabel("Adding download")
                    }
                }
            }.formStyle(.grouped).navigationTitle("Add a torrent")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }.disabled(submitting)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Add") {
                        guard !submitting, !store.busy else { return }
                        let link = canonicalMagnet(magnet)
                        let addPaused = paused
                        store.perform {
                            submitting = true
                            defer { submitting = false; showProgress = false }
                            struct AddedTorrent: Decodable { let name: String?; let hashString: String? }
                            struct AddResult: Decodable { let torrentAdded: AddedTorrent?; let torrentDuplicate: AddedTorrent? }
                            let result: AddResult = try await store.request("POST", "/v1/torrents", body: ["magnet": link, "paused": addPaused])
                            dismiss()
                            let added = result.torrentAdded ?? result.torrentDuplicate
                            let existing = store.torrents.first { $0.id == added?.hashString }
                            let state = existing.map { !$0.completed ? "Downloading" : $0.status == 6 ? "Seeding · Download complete" : "Complete" }
                            let detail = [added?.name, result.torrentDuplicate == nil ? nil : state].compactMap { $0 }.joined(separator: " · ")
                            store.notify(result.torrentDuplicate == nil ? "Torrent added" : "Torrent already in your list", detail: detail, torrentHash: added?.hashString)
                        }
                    }.disabled(!canonicalMagnet(magnet).hasPrefix("magnet:?") || submitting || store.busy || !store.connected)
                }
            }
        }
        .interactiveDismissDisabled(submitting)
        .task(id: submitting) {
            guard submitting else { showProgress = false; return }
            do { try await Task.sleep(for: .milliseconds(500)) }
            catch { return }
            guard !Task.isCancelled, submitting else { return }
            showProgress = true
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

#endif


struct BackgroundSubtitlesView: View {
    @EnvironmentObject var store: Store
    var body: some View {
        if let status = store.status {
            Toggle("Prepare translations automatically", isOn: Binding(
                get: { status.settings.backgroundSubtitles ?? true },
                set: { enabled in store.perform { try await store.send("PATCH", "/v1/settings", body: ["background_subtitles": enabled]) } }
            )).disabled(store.busy)
            if let preparation = status.subtitlePreparation {
                Text("\(preparation.translated) translated · \(preparation.scanned) scanned").font(.callout).foregroundStyle(.secondary)
                if let job = preparation.current {
                    let asset = store.library?.titles.flatMap(\.assets).first { $0.id == job.assetId }
                    Text(asset?.name ?? "Translating subtitles…").lineLimit(2)
                    if job.totalCues > 0 {
                        ProgressView(value: Double(job.completedCues), total: Double(job.totalCues))
                        Text("\(Int(Double(job.completedCues) / Double(job.totalCues) * 100))%").monospacedDigit().font(.caption)
                    }
                } else if preparation.waitingForIdle && status.settings.backgroundSubtitles != false {
                    Text("Resumes after playback").foregroundStyle(.secondary)
                }
                if preparation.failed > 0 { Text("\(preparation.failed) awaiting retry").foregroundStyle(.secondary) }
            }
        }
    }
}
