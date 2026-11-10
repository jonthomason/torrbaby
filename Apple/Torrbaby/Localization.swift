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
    static func text(_ key: String) -> String {
        guard let path = Bundle.main.path(forResource: code, ofType: "lproj"), let bundle = Bundle(path: path) else { return key }
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
        HStack(spacing: 4) {
            option("EN", code: "en")
            Text("/").foregroundStyle(.secondary)
            option("中文", code: "zh-Hans")
        }.font(.system(size: selectorFontSize, weight: .semibold))
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
            Text(verbatim: label).padding(.horizontal, 8).padding(.vertical, 6)
                .foregroundStyle(language.code == code ? Theme.canvas : Theme.mint)
                .background(language.code == code ? Theme.mint : .clear, in: RoundedRectangle(cornerRadius: 6))
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
