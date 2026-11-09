#if os(tvOS)
import SwiftUI

struct TVRootView: View {
    @EnvironmentObject var store: Store
    @Environment(\.scenePhase) var phase
    @State private var server = ""
    @State private var token = ""
    @State private var settings = false
    @State private var category = "Home"
    var body: some View {
        NavigationStack {
            if !store.configured || settings {
                VStack(alignment: .leading, spacing: 24) {
                    Image("TorrbabyLogo").resizable().frame(width: 110, height: 110)
                        .clipShape(RoundedRectangle(cornerRadius: 22)).accessibilityHidden(true)
                    Text("Meet your server.").font(.largeTitle.bold())
                    TextField("Server URL", text: $server)
                    SecureField("API token", text: $token)
                    Button("Connect") {
                        store.server = server; store.token = token
                        Task { await store.connect(); if store.connected { settings = false } }
                    }
                    if let error = store.connectionError { Text(error).foregroundStyle(.orange) }
                    if store.configured { Button("Back to library") { settings = false } }
                }.frame(maxWidth: 900).onAppear { server = store.server; token = store.token }
            } else {
                VStack {
                    Picker("Library", selection: $category) {
                        ForEach(["Home", "Movies", "TV Shows"], id: \.self) { Text($0).tag($0) }
                    }.pickerStyle(.segmented).padding(.horizontal, 60)
                    LibraryBrowser(kind: category == "Home" ? nil : category == "Movies" ? "movie" : "tv").id(category)
                }.navigationTitle("Torrbaby")
                .toolbar { Button("Settings") { settings = true } }
            }
        }
        .task(id: phase) {
            while !Task.isCancelled && phase == .active {
                await store.refresh()
                try? await Task.sleep(for: .seconds(10))
            }
        }
        .alert("Couldn’t complete the request", isPresented: Binding(get: { store.actionError != nil }, set: { if !$0 { store.actionError = nil } })) {
            Button("OK") { store.actionError = nil }
        } message: { Text(store.actionError ?? "") }
    }
}
#endif
