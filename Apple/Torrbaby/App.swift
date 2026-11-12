import SwiftUI
#if os(macOS)
import Darwin
#endif

@main
struct TorrbabyApp: App {
    init() {
        #if os(macOS)
        // A snapshot run renders Settings to files and quits; it is not a second app instance.
        if SettingsSnapshot.runIfRequested() { return }
        MacInstance.enforce()
        // Tabs are created only by an explicit New Tab action.
        NSWindow.allowsAutomaticWindowTabbing = false
        #endif
    }
    @StateObject private var store: Store = {
        #if os(macOS)
        MacInstance.migratePreferences()
        #endif
        return Store()
    }()
    #if os(iOS)
    @UIApplicationDelegateAdaptor(DownloadAppDelegate.self) private var downloadDelegate
    #endif
    #if os(macOS)
    @AppStorage("showMenuBarRates") private var showMenuBarRates = true
    #endif
    @ViewBuilder private var mainContent: some View {
        #if os(tvOS)
        LocalizedInterface { ProfileGate { TVRootView() } }.environmentObject(store).tint(Theme.accent).preferredColorScheme(.dark)
        #else
        LocalizedInterface { ProfileGate { RootView() } }
            .environmentObject(store).tint(Theme.accent).preferredColorScheme(.dark)
            .toggleStyle(MonoToggleStyle())
            #if os(iOS)
            .buttonBorderShape(.roundedRectangle(radius: 8))
            #endif
            #if os(macOS)
            .frame(minWidth: 900, minHeight: 640).background(MacLibraryWindow())
            #endif
        #endif
    }
    var body: some Scene {
        #if os(macOS)
        Window("Torrbaby", id: "main") { mainContent }
            .defaultSize(width: 1180, height: 820)
            .commands { TorrbabyCommands(store: store) }
        WindowGroup("Torrbaby", id: "library-tab") { mainContent }
            .defaultSize(width: 1180, height: 820)
        #else
        WindowGroup(id: "main") { mainContent }
        #endif
        #if os(macOS)
        Settings {
            LocalizedInterface { MacSettingsView() }.environmentObject(store).tint(Theme.accent).preferredColorScheme(.dark).toggleStyle(MonoToggleStyle())
        }
        MenuBarExtra(isInserted: $showMenuBarRates) {
            TransferMenu(store: store)
        } label: {
            Text(LocalizedStringKey(store.connected
                 ? "↓ \(rate(store.torrents.reduce(0) { $0 + $1.rateDownload }))  ↑ \(rate(store.torrents.reduce(0) { $0 + $1.rateUpload }))"
                 : "tb · Offline"))
                .monospacedDigit()
                .accessibilityLabel("Torrbaby transfer rates")
        }
        .menuBarExtraStyle(.menu)
        WindowGroup("Playback", for: Playback.self) { $playback in
            // macOS can reopen a playback window from an earlier run; its stream is long gone,
            // so such a window closes itself instead of showing a "couldn't play" error.
            if let playback, MacPlaybackWindows.opened.contains(playback.id) {
                PlayerScreen(playback: playback, store: store)
                    .preferredColorScheme(.dark)
            } else {
                StalePlaybackWindow()
            }
        }
        .windowStyle(.hiddenTitleBar)
        .defaultSize(width: 960, height: 540)
        .windowResizability(.contentMinSize)
        // A playback window is not reopened at the next launch: its stream has ended, so it could only show an error.
        .restorationBehavior(.disabled)
        #endif
    }
}

/// One monochrome look on every platform: black canvas, white selection with black
/// text, and small-radius rounded rectangles. The purple wordmark is the only colour.
enum Theme {
    static let accent = Color.white
    static let onAccent = Color.black
    static let canvas = Color.black
    static let sidebar = Color(white: 0.055)
    static let panel = Color(white: 0.09)
    static let control = Color(white: 0.16)
    static let muted = Color(white: 0.65)
    static let brand = Color(red: 0.70, green: 0.43, blue: 1.0)
    static let radius: CGFloat = 8
}

/// The app's button: white with black text when prominent, selected or (on TV) focused.
struct MonoButtonStyle: ButtonStyle {
    var prominent = false
    var compact = false
    @Environment(\.isEnabled) private var enabled
    #if os(tvOS)
    @Environment(\.isFocused) private var focused
    #else
    private let focused = false
    #endif
    func makeBody(configuration: Configuration) -> some View {
        let filled = prominent || focused
        configuration.label
            .lineLimit(1)
            .padding(.horizontal, compact ? 10 : 14).padding(.vertical, compact ? 5 : 7)
            .foregroundStyle(filled ? Theme.onAccent : Color.white)
            .background(filled ? Theme.accent : Theme.control, in: RoundedRectangle(cornerRadius: Theme.radius))
            .opacity(enabled ? (configuration.isPressed ? 0.7 : 1) : 0.4)
            .contentShape(RoundedRectangle(cornerRadius: Theme.radius))
            #if os(tvOS)
            .scaleEffect(focused ? 1.025 : 1).animation(.easeOut(duration: 0.15), value: focused)
            #endif
    }
}

#if !os(tvOS)
/// A switch in the same vocabulary: white track with a black knob when on.
struct MonoToggleStyle: ToggleStyle {
    @Environment(\.isEnabled) private var enabled
    func makeBody(configuration: Configuration) -> some View {
        HStack {
            configuration.label
            Spacer(minLength: 12)
            ZStack(alignment: configuration.isOn ? .trailing : .leading) {
                RoundedRectangle(cornerRadius: 7).fill(configuration.isOn ? Theme.accent : Theme.control).frame(width: 42, height: 24)
                RoundedRectangle(cornerRadius: 5).fill(configuration.isOn ? Theme.onAccent : Color.white).frame(width: 18, height: 18).padding(3)
            }
            .animation(.easeOut(duration: 0.12), value: configuration.isOn)
        }
        .contentShape(Rectangle())
        .onTapGesture { configuration.isOn.toggle() }
        .opacity(enabled ? 1 : 0.4)
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(.isButton)
        .accessibilityValue(Text(configuration.isOn ? "On" : "Off"))
        .accessibilityAction { configuration.isOn.toggle() }
    }
}
#endif

extension View {
    /// Forms and lists sit directly on the black canvas instead of the system's grey-blue.
    @ViewBuilder func canvasBackground() -> some View {
        #if os(tvOS)
        self.background(Theme.canvas)
        #else
        self.scrollContentBackground(.hidden).background(Theme.canvas)
        #endif
    }
}

struct Card<Content: View>: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @ViewBuilder let content: Content
    var body: some View {
        content.padding(20).frame(maxWidth: .infinity, alignment: .leading)
            .background(Theme.panel, in: RoundedRectangle(cornerRadius: 10))
            .overlay(RoundedRectangle(cornerRadius: 10).stroke(.white.opacity(0.055)))
    }
}

#if os(macOS)
private struct TransferMenu: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @ObservedObject var store: Store
    @Environment(\.openWindow) private var openWindow
    var body: some View {
        Text("Torrbaby · " + (store.connected ? "Connected" : "Offline"))
        if store.connected {
            Text("Download: " + rate(store.torrents.reduce(0) { $0 + $1.rateDownload }))
            Text("Upload: " + rate(store.torrents.reduce(0) { $0 + $1.rateUpload }))
            Text("\(store.torrents.filter { !$0.completed }.count) downloading · \(store.torrents.filter { $0.status == 6 }.count) seeding")
        }
        Divider()
        Button("Open Torrbaby") {
            if let existing = NSApp.windows.first(where: { $0.tabbingIdentifier == "torrbaby.library" }) {
                existing.deminiaturize(nil)
                existing.makeKeyAndOrderFront(nil)
            } else { openWindow(id: "main") }
            NSApp.activate(ignoringOtherApps: true)
        }
        SettingsLink()
        Button("Refresh") { Task { await store.refresh() } }.disabled(!store.configured)
        Divider()
        Button("Quit Torrbaby") { NSApp.terminate(nil) }
    }
}
#endif

#if os(macOS)
@MainActor
enum MacLibraryTabs {
    private static weak var parent: NSWindow?
    static func create(openWindow: () -> Void) {
        guard parent == nil else { return }
        parent = [NSApp.keyWindow, NSApp.mainWindow].compactMap { $0 }
            .first(where: { $0.tabbingIdentifier == "torrbaby.library" })
            ?? NSApp.windows.first(where: { $0.tabbingIdentifier == "torrbaby.library" })
        openWindow()
    }
    static func attach(_ window: NSWindow) {
        window.tabbingIdentifier = "torrbaby.library"
        let source = parent ?? NSApp.windows.first {
            $0 !== window && $0.tabbingIdentifier == "torrbaby.library" && $0.isVisible
        }
        guard let source, source !== window else { return }
        parent = nil
        // Explicit grouping is independent of macOS's automatic-tab preference.
        source.addTabbedWindow(window, ordered: .above)
        window.makeKeyAndOrderFront(nil)
    }
}

private struct MacLibraryWindow: NSViewRepresentable {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    final class Marker: NSView {
        private weak var configuredWindow: NSWindow?
        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            guard let window, window !== configuredWindow else { return }
            configuredWindow = window
            // SwiftUI must finish installing the new window before AppKit groups it.
            DispatchQueue.main.async { MacLibraryTabs.attach(window) }
        }
    }
    func makeNSView(context: Context) -> Marker { Marker() }
    func updateNSView(_ view: Marker, context: Context) {}
}

private struct AddTorrentCommandKey: FocusedValueKey { typealias Value = () -> Void }
private struct BackCommandKey: FocusedValueKey { typealias Value = () -> Void }
private struct ForwardCommandKey: FocusedValueKey { typealias Value = () -> Void }
private struct SearchCommandKey: FocusedValueKey { typealias Value = () -> Void }
extension FocusedValues {
    var goBack: (() -> Void)? {
        get { self[BackCommandKey.self] }
        set { self[BackCommandKey.self] = newValue }
    }
    var goForward: (() -> Void)? {
        get { self[ForwardCommandKey.self] }
        set { self[ForwardCommandKey.self] = newValue }
    }
    var addTorrent: (() -> Void)? {
        get { self[AddTorrentCommandKey.self] }
        set { self[AddTorrentCommandKey.self] = newValue }
    }
    var searchContent: (() -> Void)? {
        get { self[SearchCommandKey.self] }
        set { self[SearchCommandKey.self] = newValue }
    }
}
private struct TorrbabyCommands: Commands {
    @ObservedObject var store: Store
    @Environment(\.openWindow) private var openWindow
    @FocusedValue(\.goBack) private var goBack
    @FocusedValue(\.goForward) private var goForward
    @FocusedValue(\.addTorrent) private var addTorrent
    @FocusedValue(\.searchContent) private var searchContent
    var body: some Commands {
        CommandGroup(replacing: .newItem) {
            Button("New Tab") { MacLibraryTabs.create { openWindow(id: "library-tab") } }
                .keyboardShortcut("t", modifiers: .command)
            Button("Add Torrent…") { addTorrent?() }
                .keyboardShortcut("n", modifiers: [.command, .shift])
                .disabled(addTorrent == nil || !store.connected)
        }
        CommandGroup(after: .textEditing) {
            Button("Find…") { searchContent?() }.keyboardShortcut("f")
                .disabled(searchContent == nil)
        }
        CommandGroup(after: .toolbar) {
            Button("Refresh") { Task { await store.refresh() } }.keyboardShortcut("r")
                .disabled(!store.configured)
        }
        CommandMenu("History") {
            Button("Back") { goBack?() }.keyboardShortcut("[").disabled(goBack == nil)
            Button("Forward") { goForward?() }.keyboardShortcut("]").disabled(goForward == nil)
        }
        SidebarCommands()
    }
}
private struct MacSettingsView: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @State private var showConnect = false
    var body: some View {
        SettingsView(showConnect: $showConnect)
            .frame(width: 760, height: 640)
            .sheet(isPresented: $showConnect) { LocalizedInterface { ConnectionView() } }
    }
}
#endif

#if os(macOS)
/// A process-wide kernel lock also covers direct executable launches and other install paths.
@MainActor
private enum MacInstance {
    private static var descriptor: Int32 = -1
    static func migratePreferences() {
        let defaults = UserDefaults.standard
        guard let current = Bundle.main.bundleIdentifier,
              current == "com.jonthomason.torrbaby.mac",
              !defaults.bool(forKey: "migratedLegacyBundlePreferences") else { return }
        var values = defaults.persistentDomain(forName: current) ?? [:]
        for legacy in ["org.torrbaby.mac", "org.torrbaby.app"] {
            for (key, value) in defaults.persistentDomain(forName: legacy) ?? [:] where values[key] == nil {
                values[key] = value
            }
        }
        values["migratedLegacyBundlePreferences"] = true
        defaults.setPersistentDomain(values, forName: current)
    }
    static func enforce() {
        let folder = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/Torrbaby")
        do { try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true) }
        catch { fail() }
        descriptor = Darwin.open(folder.appendingPathComponent("mac-client.lock").path, O_CREAT | O_RDWR, S_IRUSR | S_IWUSR)
        guard descriptor >= 0 else { fail() }
        let acquired = flock(descriptor, LOCK_EX | LOCK_NB) == 0
        let other = NSWorkspace.shared.runningApplications.first {
            $0.processIdentifier != ProcessInfo.processInfo.processIdentifier &&
            ["com.jonthomason.torrbaby.mac", "org.torrbaby.mac", "org.torrbaby.app"].contains($0.bundleIdentifier ?? "") && !$0.isTerminated
        }
        if !acquired || other != nil {
            if let other {
                other.activate(options: [.activateAllWindows, .activateIgnoringOtherApps])
            }
            // Never unlink the lock: all copies must lock the same inode.
            exit(0)
        }
    }
    private static func fail() -> Never {
        let alert = NSAlert()
        alert.messageText = L10n.text("Couldn’t open Torrbaby")
        alert.informativeText = L10n.text("Check access to your Application Support folder and try again.")
        alert.runModal()
        exit(1)
    }
}
#endif

#if os(macOS)
/// Playback windows opened during this run of the app.
@MainActor enum MacPlaybackWindows {
    static var opened = Set<Playback.ID>()
    static func open(_ playback: Playback, with openWindow: OpenWindowAction) {
        opened.insert(playback.id)
        openWindow(value: playback)
    }
}
private struct StalePlaybackWindow: View {
    @Environment(\.dismissWindow) private var dismissWindow
    @Environment(\.dismiss) private var dismiss
    var body: some View {
        Color.black.onAppear { dismiss() }
    }
}
#endif
