import SwiftUI

/// A device preference, independent of the profile and audio/subtitle defaults.
@MainActor
final class InterfaceLanguage: ObservableObject {
    static let shared = InterfaceLanguage()
    @Published var selection: String {
        didSet { UserDefaults.standard.set(selection, forKey: "interfaceLanguage") }
    }
    private init() { selection = UserDefaults.standard.string(forKey: "interfaceLanguage") ?? "system" }
    var code: String { L10n.code }
    func choose(_ code: String) { selection = code }
}

enum L10n {
    static var code: String {
        let saved = UserDefaults.standard.string(forKey: "interfaceLanguage") ?? "system"
        if saved == "en" || saved == "zh-Hans" { return saved }
        return Locale.preferredLanguages.first?.hasPrefix("zh") == true ? "zh-Hans" : "en"
    }
    static var locale: Locale { Locale(identifier: code) }
    // Resolved once; looking the bundle up on every string was repeated per row per render.
    private static let bundles: [String: Bundle] = Dictionary(uniqueKeysWithValues: ["en", "zh-Hans"].compactMap { code in
        Bundle.main.path(forResource: code, ofType: "lproj").flatMap(Bundle.init(path:)).map { (code, $0) }
    })
    static func text(_ key: String) -> String {
        guard let bundle = bundles[code] else { return key }
        return bundle.localizedString(forKey: key, value: key, table: "Localizable")
    }
    static func format(_ key: String, _ arguments: CVarArg...) -> String {
        String(format: text(key), locale: locale, arguments: arguments)
    }
    static func language(_ code: String) -> String {
        locale.localizedString(forLanguageCode: code) ?? code
    }
}

struct InterfaceLanguagePicker: View {
    @ObservedObject private var language = InterfaceLanguage.shared
    var body: some View {
        HStack(spacing: 2) {
            option("EN", code: "en")
            option("中文", code: "zh-Hans")
        }.font(.system(size: selectorFontSize, weight: .semibold))
            .padding(3)
            .background(Theme.control, in: RoundedRectangle(cornerRadius: Theme.radius))
            .fixedSize()
            .accessibilityElement(children: .contain)
            .accessibilityLabel("Interface language")
    }
    private var selectorFontSize: CGFloat {
        #if os(tvOS)
        22
        #else
        13
        #endif
    }
    private func option(_ label: String, code: String) -> some View {
        Button { language.choose(code) } label: {
            Text(verbatim: label).frame(minWidth: 30).padding(.horizontal, 9).padding(.vertical, 5)
                .foregroundStyle(language.code == code ? Theme.onAccent : Color.white)
                .background(language.code == code ? Theme.accent : .clear, in: RoundedRectangle(cornerRadius: 6))
        }.buttonStyle(.plain)
            .accessibilityLabel(code == "en" ? "English" : "中文")
            .accessibilityAddTraits(language.code == code ? .isSelected : [])
    }
}

/// Outside the navigation content so pushing pages does not duplicate the control
/// or reset navigation/playback when the language changes.
struct LocalizedInterface<Content: View>: View {
    @ObservedObject private var language = InterfaceLanguage.shared
    @EnvironmentObject private var store: Store
    @ViewBuilder let content: Content
    var body: some View {
        content.environment(\.locale, Locale(identifier: language.code))
            .onReceive(NotificationCenter.default.publisher(for: NSLocale.currentLocaleDidChangeNotification)) { _ in
                if language.selection == "system" { language.objectWillChange.send() }
            }
    }
}

struct InterfaceLanguageSettings: View {
    @ObservedObject private var language = InterfaceLanguage.shared
    var body: some View {
        Button { language.choose("system") } label: {
            Label("Follow system language", systemImage: language.selection == "system" ? "checkmark.circle.fill" : "circle")
        }
    }
}

/// The EN/中文 switch at the trailing edge of every page's bar, without the system's
/// shared glass capsule, so it looks and sits the same whether or not a page has search.
struct LanguageToolbar: ToolbarContent {
    private var placement: ToolbarItemPlacement {
        #if os(iOS)
        .topBarTrailing
        #else
        .primaryAction
        #endif
    }
    var body: some ToolbarContent {
        #if os(tvOS)
        ToolbarItem(placement: placement) { InterfaceLanguagePicker() }
        #else
        if #available(macOS 26, iOS 26, *) {
            ToolbarItem(placement: placement) { InterfaceLanguagePicker() }.sharedBackgroundVisibility(.hidden)
        } else {
            ToolbarItem(placement: placement) { InterfaceLanguagePicker() }
        }
        #endif
    }
}

#if os(macOS)
/// A normal toolbar field leaves the language control at the trailing edge;
/// SwiftUI's searchable toolbar item otherwise always takes that position.
struct MacToolbarSearch: View {
    @Binding var text: String
    var focus: FocusState<Bool>.Binding
    let prompt: String
    var body: some View {
        HStack(spacing: 10) {
            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                TextField(LocalizedStringKey(prompt), text: $text)
                    .textFieldStyle(.plain).focused(focus)
                    .onExitCommand { text = ""; focus.wrappedValue = false }
            }.padding(.horizontal, 10).padding(.vertical, 7)
                .frame(minWidth: 180, idealWidth: 280, maxWidth: 360)
                .background(.quaternary, in: RoundedRectangle(cornerRadius: interfaceCornerRadius))
                .accessibilityLabel(LocalizedStringKey(prompt))
            InterfaceLanguagePicker()
        }
    }
}
#endif

#if os(macOS)
struct MacSearchToolbar: ToolbarContent {
    @Binding var text: String
    var focus: FocusState<Bool>.Binding
    let prompt: String
    var body: some ToolbarContent {
        if #available(macOS 26, *) {
            ToolbarItem(placement: .primaryAction) {
                MacToolbarSearch(text: $text, focus: focus, prompt: prompt)
            }.sharedBackgroundVisibility(.hidden)
        } else {
            ToolbarItem(placement: .primaryAction) {
                MacToolbarSearch(text: $text, focus: focus, prompt: prompt)
            }
        }
    }
}
#endif

#if os(iOS)
/// Search lives in the pull-down drawer on both iPhone and iPad, which keeps every
/// page's bar the same: navigation, title, page actions, language.
struct MobileLibrarySearch: ViewModifier {
    @Binding var text: String
    let prompt: String
    func body(content: Content) -> some View {
        content.searchable(text: $text, placement: .navigationBarDrawer(displayMode: .automatic), prompt: LocalizedStringKey(prompt))
    }
}
#endif

private var interfaceCornerRadius: CGFloat {
    8
}

/// Wording for the shared action alert. A title whose audio Apple devices cannot decode gets
/// its own title and a plain explanation instead of the server's internal message.
struct ActionAlert {
    let title: String
    let message: String
    init(_ error: String?, fallbackTitle: String) {
        let prefix = "Remux codec not implemented yet: "
        if let error, error.hasPrefix(prefix) {
            let codec = String(error.dropFirst(prefix.count))
            let names = ["A_DTS": "DTS", "A_TRUEHD": "Dolby TrueHD", "A_OPUS": "Opus", "A_FLAC": "FLAC", "A_VORBIS": "Vorbis"]
            title = L10n.text("Incompatible movie or TV show")
            message = L10n.format("Its audio format (%@) can’t be played on Apple devices yet.", names[codec] ?? codec.replacingOccurrences(of: "A_", with: ""))
        } else {
            title = L10n.text(fallbackTitle)
            message = L10n.text(error ?? "")
        }
    }
}
