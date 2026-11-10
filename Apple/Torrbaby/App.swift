import SwiftUI

@main
struct TorrbabyApp: App {
    init() {
        #if os(macOS)
        // Ignore the system's automatic tab preference. Explicit New Tab still works.
        NSWindow.allowsAutomaticWindowTabbing = false
        #endif
    }
    @StateObject private var store = Store()
    #if os(iOS)
    @UIApplicationDelegateAdaptor(DownloadAppDelegate.self) private var downloadDelegate
    #endif
    #if os(macOS)
    @AppStorage("showMenuBarRates") private var showMenuBarRates = true
    #endif
    var body: some Scene {
        WindowGroup(id: "main") {
            #if os(tvOS)
            LocalizedInterface { ProfileGate { TVRootView() } }.environmentObject(store).tint(Theme.mint).preferredColorScheme(.dark)
            #else
            LocalizedInterface { ProfileGate { RootView() } }
                .environmentObject(store)
                .tint(Theme.mint)
                .preferredColorScheme(.dark)
                #if os(macOS)
                .frame(minWidth: 900, minHeight: 640)
                .background(MacLibraryWindow())
                #endif
            #endif
        }
        #if os(macOS)
        .defaultSize(width: 1180, height: 820)
        .commands {
            TorrbabyCommands(store: store)
        }
        #endif
        #if os(macOS)
        Settings {
            LocalizedInterface { MacSettingsView() }.environmentObject(store).tint(Theme.mint).preferredColorScheme(.dark)
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
            if let playback {
                PlayerScreen(playback: playback, store: store)
                    .preferredColorScheme(.dark)
            }
        }
        .windowStyle(.hiddenTitleBar)
        .defaultSize(width: 960, height: 540)
        .windowResizability(.contentMinSize)
        #endif
    }
}

enum Theme {
    static let mint = Color(red: 0.44, green: 0.91, blue: 0.76)
    static let canvas = Color(red: 0.055, green: 0.075, blue: 0.10)
    static let panel = Color(red: 0.09, green: 0.115, blue: 0.15)
    static let muted = Color(red: 0.56, green: 0.63, blue: 0.69)
}

struct Card<Content: View>: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @ViewBuilder let content: Content
    var body: some View {
        content.padding(20).frame(maxWidth: .infinity, alignment: .leading)
            .background(Theme.panel, in: RoundedRectangle(cornerRadius: 18))
            .overlay(RoundedRectangle(cornerRadius: 18).stroke(.white.opacity(0.055)))
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
        guard let source = parent, source !== window else { return }
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
        CommandGroup(after: .newItem) {
            Button("New Tab") { MacLibraryTabs.create { openWindow(id: "main") } }
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
            .frame(width: 600, height: 700)
            .sheet(isPresented: $showConnect) { LocalizedInterface { ConnectionView() } }
    }
}
#endif
