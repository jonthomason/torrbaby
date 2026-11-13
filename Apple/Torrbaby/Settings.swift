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
    @ObservedObject private var activity = ServerActivity.shared
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    var body: some View { statusContent.modifier(NeedsServerStatus()) }
    @ViewBuilder private var statusContent: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 24) {
                if let status = store.status {
                    Card {
                        VStack(alignment: .leading, spacing: 18) {
                            HStack {
                                Image(systemName: status.settings.paused ? "pause.circle.fill" : "sparkles").font(.title).foregroundStyle(Theme.accent)
                                VStack(alignment: .leading, spacing: 5) {
                                    Text(LocalizedStringKey(status.settings.paused ? "Paused" : "Running")).font(.headline)
                                    Text("Checks every \(Int(status.settings.pollSeconds)) seconds").font(.caption).foregroundStyle(Theme.muted)
                                }
                                Spacer()
                                Button(LocalizedStringKey(status.settings.paused ? "Resume" : "Pause")) { store.toggleAutomation() }.buttonStyle(MonoButtonStyle())
                            }
                            if let error = status.lastError { Text(L10n.text(error)).foregroundStyle(.orange) }
                            if let error = status.scanError { Label("Library: " + error, systemImage: "exclamationmark.triangle").foregroundStyle(.orange) }
                            HStack {
                                Text("\(status.jobs.values.filter { $0.phase == "done" }.count) organized")
                                Text("·")
                                Text("\(status.pendingScans) pending scans")
                            }.font(.caption).foregroundStyle(Theme.muted)
                            HStack {
                                Button("Check now") { store.perform { try await store.send("POST", "/v1/reconcile") } }
                                Button("Scan media library") { store.perform { try await store.send("POST", "/v1/library/scan") } }
                            }.buttonStyle(MonoButtonStyle()).disabled(store.busy)
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
                                .foregroundStyle(event.error == nil ? Theme.accent : .orange).padding(.top, 2)
                            VStack(alignment: .leading, spacing: 5) {
                                Text(event.title).font(.callout.weight(.medium))
                                if let hash = event.hash, let torrent = store.torrents.first(where: { $0.id == hash }) {
                                    Text(torrent.displayName).font(.caption).foregroundStyle(Theme.muted).lineLimit(2)
                                }
                                if let error = event.error { Text(L10n.text(error)).font(.caption).foregroundStyle(.orange) }
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

/// The settings categories, in the order of iOS Settings: things about you and this device
/// first, then how it reaches the server, then the server's own settings (which apply to
/// everyone), then About. iPhone, iPad and Apple TV drill into each; the Mac shows a sidebar.
enum SettingsPage: String, CaseIterable, Identifiable {
    case user = "User", general = "General", notifications = "Notifications", playback = "Playback", downloads = "Downloads"
    case connection = "Connection", devices = "Devices"
    case library = "Library & Organizing", seeding = "Upload & Seeding", subtitles = "Subtitles & Translation", quarkDrive = "Quark Drive"
    case about = "About"
    var id: String { rawValue }
    var symbol: String {
        switch self {
        case .user: "person.crop.circle.fill"
        case .general: "gearshape.fill"
        case .notifications: "bell.badge.fill"
        case .playback: "play.rectangle.fill"
        case .downloads: "arrow.down.circle.fill"
        case .connection: "network"
        case .devices: "laptopcomputer.and.iphone"
        case .library: "folder.fill"
        case .seeding: "arrow.up.circle.fill"
        case .subtitles: "captions.bubble.fill"
        case .quarkDrive: "icloud.and.arrow.down.fill"
        case .about: "info.circle.fill"
        }
    }
    var tint: Color {
        switch self {
        case .user: .blue
        case .general, .devices, .about: .gray
        case .notifications: .red
        case .playback, .connection: .blue
        case .downloads, .seeding: .green
        case .library: .orange
        case .subtitles: .purple
        case .quarkDrive: .indigo
        }
    }
    /// Groups as they appear in the list; Downloads exists only where the app keeps offline copies.
    static var groups: [(header: String?, footer: String?, pages: [SettingsPage])] {
        [(nil, nil, [.user]),
         (nil, nil, [.general, .notifications, .playback] + (downloadsAvailable ? [.downloads] : [])),
         (nil, nil, [.connection, .devices]),
         ("Server", "These settings apply to everyone who uses this server.", [.library, .seeding, .subtitles, .quarkDrive]),
         (nil, nil, [.about])]
    }
    static var downloadsAvailable: Bool {
        #if os(tvOS)
        false
        #else
        true
        #endif
    }
}

/// The coloured rounded-square icon iOS Settings uses for each row.
struct SettingsIcon: View {
    let page: SettingsPage
    var body: some View {
        Image(systemName: page.symbol).font(.system(size: 15, weight: .semibold)).foregroundStyle(.white)
            .frame(width: 29, height: 29).background(page.tint.gradient, in: RoundedRectangle(cornerRadius: 7))
            .accessibilityHidden(true)
    }
}

/// The two things Settings is opened for most, kept above everything else: linking
/// another device, and which build this is.
private struct SettingsQuickRows: View {
    @EnvironmentObject var store: Store
    @State private var showPairing = false
    private func icon(_ symbol: String, _ tint: Color) -> some View {
        Image(systemName: symbol).font(.system(size: 15, weight: .semibold)).foregroundStyle(.white)
            .frame(width: 29, height: 29).background(tint.gradient, in: RoundedRectangle(cornerRadius: 7))
            .accessibilityHidden(true)
    }
    var body: some View {
        Button { showPairing = true } label: {
            Label { Text("Link a device").foregroundStyle(Color.primary) } icon: { icon("qrcode", .indigo) }
        }
        .buttonStyle(.plain)
        .disabled(!store.connected)
        .sheet(isPresented: $showPairing) { LocalizedInterface { PairingCodeView() } }
        #if os(macOS)
        // The Mac sidebar is narrow: the build sits under its title instead of beside it.
        Label {
            VStack(alignment: .leading, spacing: 1) {
                Text("Version")
                Text(verbatim: AppRelease.description.replacingOccurrences(of: "Version ", with: "")).font(.caption).monospacedDigit()
                    .foregroundStyle(.secondary).textSelection(.enabled)
            }
        } icon: { icon("info", .gray) }
        #else
        LabeledContent {
            Text(verbatim: AppRelease.description.replacingOccurrences(of: "Version ", with: "")).monospacedDigit().textSelection(.enabled)
        } label: {
            Label { Text("Version") } icon: { icon("info", .gray) }
        }
        #endif
    }
}

struct SettingsView: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    @Binding var showConnect: Bool
    #if os(macOS)
    @State private var selected: SettingsPage? = SettingsView.initialPage
    /// The page a new Settings window opens on.
    static var initialPage: SettingsPage = .general
    #endif
    var body: some View {
        #if os(macOS)
        // System Settings on the Mac: categories on the left, the chosen page on the right.
        HStack(spacing: 0) {
            List(selection: $selected) {
                SwiftUI.Section { SettingsQuickRows() }
                ForEach(Array(SettingsPage.groups.enumerated()), id: \.offset) { _, group in
                    SwiftUI.Section {
                        ForEach(group.pages) { page in
                            Label { Text(LocalizedStringKey(page.rawValue)) } icon: { SettingsIcon(page: page) }.tag(page)
                        }
                    } header: { if let header = group.header { Text(LocalizedStringKey(header)) } }
                }
            }.listStyle(.sidebar).frame(width: 220)
            Divider()
            SettingsPageView(page: selected ?? .general, showConnect: $showConnect)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }.background(Theme.canvas)
        #else
        List {
            SwiftUI.Section { SettingsQuickRows() }
            ForEach(Array(SettingsPage.groups.enumerated()), id: \.offset) { _, group in
                SwiftUI.Section {
                    ForEach(group.pages) { page in
                        NavigationLink {
                            SettingsPageView(page: page, showConnect: $showConnect)
                                .navigationTitle(L10n.text(page == .user ? (store.activeProfile?.name ?? "User") : page.rawValue))
                        } label: {
                            if page == .user {
                                HStack(spacing: 14) {
                                    if let profile = store.activeProfile { ProfileAvatar(profile: profile, size: 48) }
                                    else { Image(systemName: page.symbol).font(.system(size: 44)).foregroundStyle(page.tint).accessibilityHidden(true) }
                                    VStack(alignment: .leading, spacing: 2) {
                                        Text(verbatim: store.activeProfile?.name ?? L10n.text("Choose a user")).font(.title3.weight(.semibold))
                                        Text("Switch user").font(.subheadline).foregroundStyle(.secondary)
                                    }
                                }.padding(.vertical, 4)
                            } else {
                                Label { Text(LocalizedStringKey(page.rawValue)) } icon: { SettingsIcon(page: page) }
                            }
                        }
                    }
                } header: { if let header = group.header { Text(LocalizedStringKey(header)) } }
                  footer: { if let footer = group.footer { Text(LocalizedStringKey(footer)) } }
            }
        }
        #if os(iOS)
        .listStyle(.insetGrouped)
        #endif
        .canvasBackground()
        #endif
    }
}

/// One settings page. Changes take effect at once, as in iOS Settings, except where a page
/// edits server folders or seeding, which need an explicit Save because they move files.
struct SettingsPageView: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    let page: SettingsPage
    @Binding var showConnect: Bool
    var body: some View {
        Form {
            switch page {
            case .user: UserSettings()
            case .general: GeneralSettings()
            case .notifications: NotificationSettings()
            case .playback: PlaybackSettings()
            case .downloads: DownloadPolicySettings()
            case .connection: ConnectionSettings(showConnect: $showConnect)
            case .devices: DeviceSettings()
            case .library: LibrarySettings()
            case .seeding: SeedingSettings()
            case .quarkDrive: QuarkDriveSettings()
            case .subtitles: SwiftUI.Section { BackgroundSubtitlesView() } footer: { Text("Prepares English and Chinese subtitles for new titles on your server, on the server itself.") }
            case .about: AboutSettings()
            }
        }.formStyle(.grouped).canvasBackground().tint(Theme.accent)
        #if !os(macOS)
        .toolbar { LanguageToolbar() }
        #endif
    }
}

/// A row that opens a sheet, drawn like a navigation row so it reads as tappable.
struct SettingsDisclosureRow: View {
    let title: String
    let action: () -> Void
    var body: some View {
        Button(action: action) {
            HStack {
                Text(LocalizedStringKey(title)).foregroundStyle(.primary)
                Spacer()
                Image(systemName: "chevron.right").font(.footnote.weight(.semibold)).foregroundStyle(.tertiary)
            }.contentShape(Rectangle())
        }.buttonStyle(.plain)
    }
}

private struct UserSettings: View {
    @EnvironmentObject var store: Store
    #if os(macOS)
    @Environment(\.openWindow) private var openWindow
    #endif
    @State private var editing: UserProfile?
    var body: some View {
        SwiftUI.Section {
            if let profile = store.activeProfile {
                HStack(spacing: 14) {
                    ProfileAvatar(profile: profile, size: 52)
                    Text(verbatim: profile.name).font(.title3.weight(.semibold))
                }.padding(.vertical, 4)
                // The sheet hangs on this one button. On the section it was applied to every
                // row, and several sheets opening at once closed each other straight away.
                Button("Edit profile") { editing = profile }
                    .sheet(item: $editing) { person in LocalizedInterface { ProfileEditor(profile: person) }.environmentObject(store) }
            } else {
                LabeledContent { Text("Choose a user") } label: { Text("User") }
            }
            Button("Switch user") {
                store.choosingProfile = true
                #if os(macOS)
                openWindow(id: "main")
                NSApp.activate(ignoringOtherApps: true)
                #endif
            }
        } footer: { Text("Each user has their own Continue Watching and viewing history.") }
    }
}

private struct GeneralSettings: View {
    #if os(macOS)
    @AppStorage("showMenuBarRates") private var showMenuBarRates = true
    @State private var magnetStatus: String?
    #endif
    var body: some View {
        SwiftUI.Section("Interface language") { InterfaceLanguageSettings() }
        #if os(macOS)
        SwiftUI.Section("Menu bar") {
            Toggle("Show download and upload rates", isOn: $showMenuBarRates)
        }
        SwiftUI.Section("Magnet links") {
            Button("Use Torrbaby for magnet links") {
                NSWorkspace.shared.setDefaultApplication(at: Bundle.main.bundleURL, toOpenURLsWithScheme: "magnet") { error in
                    Task { @MainActor in
                        magnetStatus = error?.localizedDescription ?? "Torrbaby is now your default magnet-link app."
                    }
                }
            }
            if let magnetStatus { Text(L10n.text(magnetStatus)).font(.caption).foregroundStyle(.secondary) }
        }
        #endif
    }
}

private struct NotificationSettings: View {
    @AppStorage("activityAlerts") private var activityAlerts = true
    @State private var notificationStatus: String?
    var body: some View {
        SwiftUI.Section {
            Toggle("Download and seeding updates", isOn: $activityAlerts)
        }
        #if os(iOS)
        SwiftUI.Section {
            Button("Enable device notifications") {
                Task {
                    do {
                        let allowed = try await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound])
                        notificationStatus = allowed ? "Notifications enabled" : "Allow notifications in iOS Settings."
                    } catch { notificationStatus = error.localizedDescription }
                }
            }
        } footer: { if let notificationStatus { Text(L10n.text(notificationStatus)) } }
        #endif
    }
}

private struct PlaybackSettings: View {
    @State private var showSubtitles = false
    var body: some View {
        SwiftUI.Section {
            SettingsDisclosureRow(title: "Default subtitles") { showSubtitles = true }
                .sheet(isPresented: $showSubtitles) { LocalizedInterface { SubtitlePreferencesView() } }
        } footer: { Text("Subtitle choices belong to this device.") }
    }
}

private struct ConnectionSettings: View {
    @EnvironmentObject var store: Store
    @Binding var showConnect: Bool
    var body: some View {
        // Routes are set at pairing and kept up to date by the server; there is nothing to edit.
        SwiftUI.Section {
            LabeledContent { Text(LocalizedStringKey(store.connectionLabel)) } label: { Text("Status") }
            LabeledContent("Server", value: store.server.isEmpty ? L10n.text("Not configured") : store.serverName.isEmpty ? store.primaryAddress : store.serverName)
        }
        SwiftUI.Section { Button("Edit connection") { showConnect = true } }
    }
}

private struct DeviceSettings: View {
    @EnvironmentObject var store: Store
    @State private var showPairing = false
    var body: some View {
        SwiftUI.Section {
            Button { showPairing = true } label: { Label("Link a device", systemImage: "qrcode") }
                .disabled(!store.connected)
                .sheet(isPresented: $showPairing) { LocalizedInterface { PairingCodeView() } }
        }
        SwiftUI.Section("Devices") {
            ForEach(store.clients) { client in
                DeviceNameRow(client: client, isCurrent: client.id == store.clientId)
            }
        }
    }
}

private struct LibrarySettings: View {
    @ObservedObject private var activity = ServerActivity.shared
    @EnvironmentObject var store: Store
    @State private var source = ""
    @State private var movies = ""
    @State private var tv = ""
    @State private var interval = 30.0
    @State private var saved = false
    var body: some View { statusContent.modifier(NeedsServerStatus()) }
    @ViewBuilder private var statusContent: some View {
        Group {
            SwiftUI.Section {
                if let status = store.status {
                    Toggle("Organize completed downloads", isOn: Binding(get: { !status.settings.paused }, set: { _ in store.toggleAutomation() })).disabled(store.busy)
                }
                Stepper("Check every \(Int(interval)) seconds", value: $interval, in: 5...3600, step: 5)
            }
            DrivesSettings()
            // The single set of folders from before drives; with a drive list, each drive has its own.
            if !(store.status?.settings.source.isEmpty ?? true) {
            SwiftUI.Section("Folders on your server") {
                ForEach([("Downloads", $source), ("Movies", $movies), ("TV shows", $tv)], id: \.0) { label, binding in
                    LabeledContent(LocalizedStringKey(label)) {
                        TextField(LocalizedStringKey(label), text: binding).labelsHidden().multilineTextAlignment(.trailing)
                            #if os(iOS)
                            .textInputAutocapitalization(.never).autocorrectionDisabled()
                            #endif
                    }
                }
            }
            }
            SwiftUI.Section {
                Button(LocalizedStringKey(saved ? "Saved" : "Save settings")) {
                    store.perform {
                        let folders = !(store.status?.settings.source.isEmpty ?? true)
                        try await store.send("PATCH", "/v1/settings", body: folders ? ["source": source, "movies": movies, "tv": tv, "poll_seconds": interval] : ["poll_seconds": interval])
                        saved = true
                    }
                }.disabled(store.busy || !store.connected)
            }
        }
        .onAppear {
            if let s = store.status?.settings { source = s.source; movies = s.movies; tv = s.tv; interval = s.pollSeconds }
        }
        .onChange(of: source) { _, _ in saved = false }
        .onChange(of: movies) { _, _ in saved = false }
        .onChange(of: tv) { _, _ in saved = false }
        .onChange(of: interval) { _, _ in saved = false }
    }
}

private struct AboutSettings: View {
    @ObservedObject private var activity = ServerActivity.shared
    @EnvironmentObject var store: Store
    var body: some View { statusContent.modifier(NeedsServerStatus()) }
    @ViewBuilder private var statusContent: some View {
        SwiftUI.Section {
            LabeledContent { Text(verbatim: AppRelease.description.replacingOccurrences(of: "Version ", with: "")).monospacedDigit() } label: { Text("Version") }
            if let status = store.status { LabeledContent { Text(verbatim: status.version).monospacedDigit() } label: { Text("Server version") } }
            LabeledContent { Text("MIT License") } label: { Text("License") }
        }
    }
}

struct SeedingSettings: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    @State private var available = false
    @State private var enabled = true
    @State private var ratio = 1.2
    @State private var saved = false
    struct Policy: Decodable { let enabled: Bool; let ratio: Double }
    var body: some View {
        Group {
            if !available {
                SwiftUI.Section { HStack { Spacer(); ProgressView(); Spacer() } }
            } else {
                SwiftUI.Section {
                    Toggle("Move to Complete at target ratio", isOn: $enabled)
                    LabeledContent("Seeding target") {
                        TextField("Target upload/download ratio", value: $ratio, format: .number.precision(.fractionLength(1...2)))
                            .labelsHidden().multilineTextAlignment(.trailing)
                    }.disabled(!enabled)
                } footer: { Text("Individual torrent limits take precedence.") }
                SwiftUI.Section {
                    Button(LocalizedStringKey(saved ? "Saved" : "Save seeding settings")) {
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

/// The household's Quark Drive (夸克网盘) account, which mainland titles in Find are fetched
/// through. The login cookie goes to the user's own server and is never shown again.
struct QuarkDriveSettings: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    struct Account: Decodable {
        let configured: Bool; var signedIn: Bool? = nil; var member: String? = nil
        var usedBytes: Int64? = nil; var totalBytes: Int64? = nil; var error: String? = nil
    }
    @State private var account: Account?
    @State private var loaded = false
    @State private var cookie = ""
    private var status: String {
        guard let account, account.configured else { return "Not set up" }
        return account.signedIn == false ? "Signed out — enter a new login cookie" : "Signed in"
    }
    var body: some View {
        Group {
            if !loaded {
                SwiftUI.Section { HStack { Spacer(); ProgressView(); Spacer() } }
            } else {
                SwiftUI.Section {
                    LabeledContent("Status") {
                        Text(LocalizedStringKey(status)).foregroundStyle(account?.signedIn == false ? .orange : .secondary)
                    }
                    if let member = account?.member, !member.isEmpty {
                        LabeledContent("Membership") { Text(verbatim: member) }
                    }
                    if let used = account?.usedBytes, let total = account?.totalBytes, total > 0 {
                        LabeledContent("Space used") {
                            Text(L10n.format("%@ of %@", ByteCountFormatter.string(fromByteCount: used, countStyle: .file), ByteCountFormatter.string(fromByteCount: total, countStyle: .file)))
                                .monospacedDigit()
                        }
                    }
                    if let error = account?.error, !error.isEmpty { Text(verbatim: error).font(.caption).foregroundStyle(.orange) }
                    LabeledContent("Login cookie") {
                        SecureField(account?.configured == true ? L10n.text("Unchanged") : "", text: $cookie).labelsHidden().multilineTextAlignment(.trailing)
                            .autocorrectionDisabled()
                    }
                } footer: {
                    Text("Find uses this account for titles under China. Torrbaby saves a share's videos to the account, downloads them into your library, then deletes its copy from the account. The cookie is kept on your server.")
                }
                SwiftUI.Section {
                    Button("Save Quark login") {
                        store.perform { account = try await store.request("PUT", "/v1/cloud/quark", body: ["cookie": cookie.trimmingCharacters(in: .whitespacesAndNewlines)], timeout: 30); cookie = "" }
                    }.disabled(store.busy || !store.connected || cookie.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    if account?.configured == true {
                        Button("Remove Quark login", role: .destructive) {
                            store.perform { account = try await store.request("DELETE", "/v1/cloud/quark") }
                        }.disabled(store.busy || !store.connected)
                    }
                }
            }
        }
        .task(id: store.server) {
            account = try? await store.request("GET", "/v1/cloud/quark", timeout: 30)
            loaded = true
        }
    }
}

struct ConnectionView: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
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
                    .clipShape(RoundedRectangle(cornerRadius: 10)).accessibilityHidden(true)
                HStack(spacing: 20) {
                    ZStack {
                        Image(systemName: "desktopcomputer").font(.system(size: 76, weight: .light))
                        Image(systemName: "qrcode").font(.system(size: 27)).offset(y: -8)
                    }
                    Image(systemName: "arrow.right").font(.title3).foregroundStyle(Theme.muted)
                    Image(systemName: "iphone.gen3.radiowaves.left.and.right")
                        .font(.system(size: 58, weight: .light))
                }.foregroundStyle(Theme.accent).padding(.vertical, 12).accessibilityHidden(true)
                VStack(spacing: 8) {
                    Text("Link a device").font(.system(size: 28, weight: .bold, design: .rounded))
                    Text("On your connected device").font(.subheadline).foregroundStyle(Theme.muted)
                    Label("Settings → Link a device", systemImage: "gearshape").font(.subheadline)
                }.multilineTextAlignment(.center)
                Button { showScanner = true } label: {
                    Label("Scan connection code", systemImage: "qrcode.viewfinder")
                        .font(.headline).padding(.horizontal, 12).padding(.vertical, 10)
                }
                .buttonStyle(MonoButtonStyle(prominent: true)).foregroundStyle(Theme.onAccent)
                .sheet(isPresented: $showScanner) { LocalizedInterface { PairingScannerView { dismiss() } } }
                Spacer()
                Spacer().frame(height: 40)
            }.padding(30).frame(maxWidth: .infinity, maxHeight: .infinity)
                .background(Theme.canvas)
                .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Close") { dismiss() } } }
            #else
            VStack(alignment: .leading, spacing: 24) {
                Image(systemName: "arrow.down.forward.and.arrow.up.backward.circle.fill").font(.system(size: 55)).foregroundStyle(Theme.accent)
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
                if let error = store.connectionError { Text(L10n.text(error)).font(.caption).foregroundStyle(.orange) }
                Button {
                    store.server = address; store.token = token
                    Task { await store.connect(); if store.connected { dismiss() } }
                } label: {
                    HStack { Spacer(); if store.isConnecting { ProgressView().controlSize(.small) }; Text(LocalizedStringKey(store.isConnecting ? "Connecting…" : "Connect to server")).fontWeight(.semibold); Spacer() }.padding(.vertical, 7)
                }.buttonStyle(MonoButtonStyle(prominent: true)).foregroundStyle(Theme.onAccent).disabled(address.isEmpty || token.isEmpty || store.isConnecting)
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
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
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
            }.formStyle(.grouped).canvasBackground().navigationTitle("Add a torrent")
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
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    let client: ClientDevice
    let isCurrent: Bool
    @State private var name = ""
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            TextField(client.platform, text: $name).font(.body)
            HStack(spacing: 16) {
                if isCurrent { Text("This device").font(.caption).foregroundStyle(.secondary) }
                Spacer()
                if name != client.name {
                    Button("Save") {
                        store.perform { try await store.send("PATCH", "/v1/clients/" + client.id, body: ["name": name]) }
                    }.disabled(name.trimmingCharacters(in: .whitespaces).isEmpty || store.busy)
                }
                if !isCurrent && client.pairedCredential == 1 {
                    Button("Revoke access", role: .destructive) {
                        store.perform { try await store.send("DELETE", "/v1/device-credentials/" + client.id) }
                    }
                }
            }.font(.caption).buttonStyle(.borderless)
        }.onAppear { name = client.name }
        .onChange(of: client.name) { _, value in name = value }
    }
}

#endif


struct BackgroundSubtitlesView: View {
    @ObservedObject private var activity = ServerActivity.shared
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    var body: some View { statusContent.modifier(NeedsServerStatus()) }
    @ViewBuilder private var statusContent: some View {
        if let status = store.status {
            Toggle("Process library automatically", isOn: Binding(
                get: { status.settings.backgroundSubtitles ?? true },
                set: { enabled in store.perform { try await store.send("PATCH", "/v1/settings", body: ["background_subtitles": enabled]) } }
            )).disabled(store.busy)
            if let preparation = status.subtitlePreparation {
                VStack(alignment: .leading, spacing: 6) {
                    Text("\(preparation.translated) processed · \(preparation.scanned) scanned").font(.callout).foregroundStyle(.secondary)
                    if let job = preparation.current {
                        let asset = store.library?.titles.flatMap(\.assets).first { $0.id == job.assetId }
                        HStack {
                            Text(LocalizedStringKey(asset?.name ?? "Translating subtitles…")).lineLimit(2)
                            Spacer()
                            if job.totalCues > 0 { Text("\(Int(Double(job.completedCues) / Double(job.totalCues) * 100))%").monospacedDigit().font(.caption).foregroundStyle(.secondary) }
                        }
                        if job.totalCues > 0 { ProgressView(value: Double(job.completedCues), total: Double(job.totalCues)) }
                    } else if preparation.waitingForIdle && status.settings.backgroundSubtitles != false {
                        Text("Resumes after playback").foregroundStyle(.secondary)
                    }
                    if preparation.failed > 0 { Text("\(preparation.failed) awaiting retry").foregroundStyle(.secondary) }
                }.padding(.vertical, 2)
            }
        }
    }
}


/// Processing errors and skipped-stage warnings in Needs attention, each dismissable as seen.
struct ProcessingAttention: View {
    @ObservedObject private var activity = ServerActivity.shared
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    @Environment(\.scenePhase) private var phase
    @Binding var count: Int
    @State private var items: [AttentionItem] = []
    private func load() async {
        guard store.connected, let list: AttentionList = try? await store.request("GET", "/v1/attention", timeout: 8) else { return }
        items = list.items; count = list.items.count
    }
    private func dismiss(_ body: [String: Any]) {
        store.perform {
            try await store.send("POST", "/v1/attention/dismiss", body: body)
            await load()
        }
    }
    var body: some View { statusContent.modifier(NeedsServerStatus()) }
    @ViewBuilder private var statusContent: some View {
        Group {
            if !items.isEmpty {
                VStack(alignment: .leading, spacing: 10) {
                    HStack {
                        Text("\(L10n.text("Processing")) · \(items.count)").font(.caption.weight(.semibold)).tracking(1.6).foregroundStyle(Theme.muted)
                        Spacer()
                        Button("Dismiss all") { dismiss(["all": true]) }.font(.caption.weight(.semibold)).disabled(store.busy)
                    }
                    ForEach(items) { item in
                        #if os(tvOS)
                        Button { dismiss(["ids": [item.id]]) } label: {
                            VStack(alignment: .leading, spacing: 6) {
                                Label { Text(verbatim: item.title).lineLimit(1) } icon: { Image(systemName: item.severity == "error" ? "exclamationmark.triangle.fill" : "info.circle") }
                                Text(LocalizedStringKey(item.summary)).font(.caption)
                                Text(L10n.text(item.error)).font(.caption2).foregroundStyle(.secondary).lineLimit(2)
                                Text("Select to dismiss").font(.caption2).foregroundStyle(.secondary)
                            }.frame(maxWidth: .infinity, alignment: .leading)
                        }
                        #else
                        HStack(alignment: .top, spacing: 12) {
                            Image(systemName: item.severity == "error" ? "exclamationmark.triangle.fill" : "info.circle")
                                .foregroundStyle(item.severity == "error" ? .orange : Theme.muted).padding(.top, 2)
                            VStack(alignment: .leading, spacing: 3) {
                                Text(verbatim: item.title).font(.subheadline.weight(.semibold)).lineLimit(1).truncationMode(.middle)
                                Text(LocalizedStringKey(item.summary)).font(.caption)
                                Text(L10n.text(item.error)).font(.caption).foregroundStyle(.secondary).lineLimit(3)
                            }
                            Spacer(minLength: 8)
                            Button { dismiss(["ids": [item.id]]) } label: { Image(systemName: "xmark.circle.fill").font(.title3) }
                                .buttonStyle(.plain).foregroundStyle(Theme.muted)
                                .accessibilityLabel(L10n.text("Dismiss"))
                                #if os(macOS)
                                .help(L10n.text("Dismiss"))
                                #endif
                        }.padding(12).background(Theme.panel, in: RoundedRectangle(cornerRadius: 10))
                        #endif
                    }
                }
            }
        }
        .task {
            while !Task.isCancelled {
                if phase == .active { await load() }
                do { try await Task.sleep(for: .seconds(15)) } catch { return }
            }
        }
    }
}

#if !os(tvOS)
#endif

#if os(macOS)
/// Test aid: `-snapshotSettings <folder>` renders every Mac Settings page, in English and
/// Chinese, to PNG files and quits. It lets the layout be checked on a Mac whose screen
/// cannot be captured. Available in release builds because only the signed app can read
/// its saved connection.
@MainActor enum SettingsSnapshot {
    @discardableResult static func runIfRequested() -> Bool {
        guard let folder = UserDefaults.standard.string(forKey: "snapshotSettings") else { return false }
        Task { @MainActor in
            // Its own store: with the screen locked the app may never show a window to borrow one from.
            let store = Store()
            await store.connect()
            try? await store.resolveProfile()
            await store.refresh(polling: false)
            for _ in 0..<20 where !store.connected || store.status == nil { try? await Task.sleep(for: .milliseconds(500)) }
            for language in ["en", "zh-Hans"] {
                InterfaceLanguage.shared.selection = language
                for page in SettingsPage.allCases {
                    SettingsView.initialPage = page
                    let size = NSSize(width: 760, height: 640)
                    let view = LocalizedInterface { SettingsView(showConnect: .constant(false)) }
                        .environmentObject(store).tint(Theme.accent).preferredColorScheme(.dark)
                        .toggleStyle(MonoToggleStyle()).frame(width: size.width, height: size.height)
                    let host = NSHostingView(rootView: view)
                    host.frame = NSRect(origin: .zero, size: size)
                    let window = NSWindow(contentRect: host.frame, styleMask: [.titled], backing: .buffered, defer: false)
                    window.isReleasedWhenClosed = false
                    window.appearance = NSAppearance(named: .darkAqua)
                    window.contentView = host
                    window.setFrameOrigin(NSPoint(x: 40, y: 40)); window.orderFrontRegardless()
                    try? await Task.sleep(for: .seconds(3))
                    host.layoutSubtreeIfNeeded()
                    if let image = host.bitmapImageRepForCachingDisplay(in: host.bounds) {
                        host.cacheDisplay(in: host.bounds, to: image)
                        let name = "mac-\(language)-\(page.rawValue.replacingOccurrences(of: " & ", with: "-")).png"
                        try? image.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: folder).appendingPathComponent(name))
                    }
                    window.close()
                }
            }
            SettingsView.initialPage = .general
            NSApp.terminate(nil)
        }
        return true
    }
}
#endif
