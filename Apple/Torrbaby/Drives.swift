import SwiftUI

/// The drives the server keeps media on (Settings → Library) and moving a title between
/// them (Manage Files). Drives are administration: nothing else in the app names them.
struct ServerDrive: Decodable, Identifiable {
    let id: String
    let name: String
    let status: String
    let totalBytes: Int64
    let freeBytes: Int64
    let acceptNew: Bool
    let reserveBytes: Int64
    let files: Int
    let bytes: Int64
    let full: Bool
    var connected: Bool { status == "online" }
    /// What its state means, as a short line.
    var state: String {
        switch status {
        case "online": return full ? "Full · new downloads go to other drives" : (acceptNew ? "Connected" : "Connected · not used for new downloads")
        case "foreign": return "Another drive is connected in its place"
        case "elsewhere": return "Connected under another name"
        default: return "Not connected"
        }
    }
}
private struct ServerDrives: Decodable { let drives: [ServerDrive] }
private struct DriveCandidate: Decodable, Identifiable {
    let mount: String
    let name: String
    let totalBytes: Int64
    let freeBytes: Int64
    let writable: Bool
    let usedBefore: Bool
    var id: String { mount }
}
private struct DriveCandidates: Decodable { let candidates: [DriveCandidate] }
struct DriveMove: Decodable, Identifiable {
    let id: String
    let drive: String
    let assetIds: [String]
    let state: String
    let bytes: Int64
    let doneBytes: Int64
    let error: String?
    let waiting: String?
    var active: Bool { state == "queued" || state == "running" }
    var fraction: Double { bytes > 0 ? min(1, Double(doneBytes) / Double(bytes)) : 0 }
}
private struct DriveMoves: Decodable { let moves: [DriveMove] }
private struct DriveMovePreview: Decodable {
    struct Choice: Decodable, Identifiable { let id: String; let name: String; let online: Bool; let freeBytes: Int64; let bytes: Int64; let fits: Bool }
    let choices: [Choice]
    let moving: Bool
}
private struct DriveMoveStarted: Decodable { let id: String }


/// Settings → Library → Drives.
struct DrivesSettings: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    @State private var drives: [ServerDrive] = []
    @State private var moves: [DriveMove] = []
    @State private var loaded = false
    @State private var adding = false
    @State private var editing: ServerDrive?
    private func load() async {
        guard store.connected else { return }
        if let list: ServerDrives = try? await store.request("GET", "/v1/drives") { drives = list.drives; loaded = true }
        if let list: DriveMoves = try? await store.request("GET", "/v1/drives/moves") { moves = list.moves.filter(\.active) }
    }
    var body: some View {
        SwiftUI.Section {
            if !loaded { HStack { Spacer(); ProgressView(); Spacer() } }
            ForEach(drives) { drive in
                Button { editing = drive } label: {
                    HStack(spacing: 12) {
                        Image(systemName: drive.connected ? "externaldrive.fill" : "externaldrive.badge.xmark")
                            .foregroundStyle(drive.connected && !drive.full ? Theme.accent : .orange).frame(width: 24)
                        VStack(alignment: .leading, spacing: 3) {
                            Text(verbatim: drive.name).foregroundStyle(.primary)
                            Text(LocalizedStringKey(drive.state)).font(.caption).foregroundStyle(drive.connected && !drive.full ? Theme.muted : .orange)
                            if drive.connected {
                                Text(L10n.format("%@ free of %@", bytes(drive.freeBytes), bytes(drive.totalBytes))).font(.caption).foregroundStyle(Theme.muted).monospacedDigit()
                            }
                        }
                        Spacer()
                        Image(systemName: "chevron.right").font(.footnote.weight(.semibold)).foregroundStyle(.tertiary)
                    }.contentShape(Rectangle())
                }.buttonStyle(.plain)
            }
            ForEach(moves) { move in
                VStack(alignment: .leading, spacing: 6) {
                    Text(L10n.format("Moving to %@ · %lld%%", drives.first { $0.id == move.drive }?.name ?? move.drive, Int64(move.fraction * 100))).font(.caption)
                    ProgressView(value: move.fraction)
                    if let waiting = move.waiting { Text(L10n.text(waiting)).font(.caption).foregroundStyle(Theme.muted) }
                }
            }
            if loaded { Button("Add Drive…") { adding = true }.disabled(!store.connected) }
        } header: { Text("Drives") } footer: {
            Text("New episodes go to the drive that already has the show. Everything else goes to the drive with the most free space.")
        }
        .task(id: store.server) {
            while !Task.isCancelled {
                await load()
                do { try await Task.sleep(for: .seconds(moves.isEmpty ? 30 : 3)) } catch { return }
            }
        }
        .sheet(isPresented: $adding) { AddDriveView { await load() } }
        .sheet(item: $editing) { drive in DriveDetailView(drive: drive, count: drives.count) { await load() } }
    }
}

/// One drive: its name, whether new downloads may go there, the space it keeps free.
private struct DriveDetailView: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) private var dismiss
    @State var drive: ServerDrive
    let count: Int
    let changed: () async -> Void
    @State private var name = ""
    @State private var confirmRemove = false
    @State private var error: String?
    private static let reserves: [Int64] = [10, 25, 50, 100, 200, 500].map { $0 * 1_000_000_000 }
    private func save(_ body: [String: Any]) {
        store.perform {
            drive = try await store.request("PATCH", "/v1/drives/\(drive.id)", body: body)
            await changed()
        }
    }
    var body: some View {
        NavigationStack {
            Form {
                SwiftUI.Section {
                    LabeledContent("Name") {
                        TextField("Name", text: $name).labelsHidden().multilineTextAlignment(.trailing)
                            .onSubmit { if name != drive.name { save(["name": name]) } }
                    }
                    LabeledContent("Status") { Text(LocalizedStringKey(drive.state)).foregroundStyle(drive.connected && !drive.full ? Theme.muted : .orange) }
                    if drive.connected {
                        LabeledContent("Space") { Text(L10n.format("%@ free of %@", bytes(drive.freeBytes), bytes(drive.totalBytes))).monospacedDigit() }
                    }
                    LabeledContent("On this drive") { Text(L10n.format("%@ · %lld files", bytes(drive.bytes), Int64(drive.files))).monospacedDigit() }
                }
                SwiftUI.Section {
                    Toggle("Use for new downloads", isOn: Binding(get: { drive.acceptNew }, set: { save(["accept_new": $0]) })).disabled(store.busy)
                    Picker("Keep free", selection: Binding(get: { drive.reserveBytes }, set: { save(["reserve_bytes": $0]) })) {
                        ForEach(Self.reserves + (Self.reserves.contains(drive.reserveBytes) ? [] : [drive.reserveBytes]), id: \.self) { Text(verbatim: bytes($0)).tag($0) }
                    }.disabled(store.busy)
                } footer: { Text("New downloads stop going to a drive once its free space falls to this amount.") }
                SwiftUI.Section {
                    Button("Remove Drive", role: .destructive) { confirmRemove = true }
                        .disabled(store.busy || drive.files > 0 || count < 2)
                } footer: {
                    Text(LocalizedStringKey(drive.files > 0 ? "To remove this drive, first move its titles to another drive from each title’s Manage Files." : "Its files stay on the drive. Torrbaby stops using it."))
                }
                if let error { Text(L10n.text(error)).foregroundStyle(.orange) }
            }.formStyle(.grouped)
            .navigationTitle(Text(verbatim: drive.name))
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") {
                        if name != drive.name { save(["name": name]) }
                        dismiss()
                    }
                }
            }
            .confirmationDialog("Remove this drive?", isPresented: $confirmRemove, titleVisibility: .visible) {
                Button("Remove Drive", role: .destructive) {
                    Task {
                        do { try await store.send("DELETE", "/v1/drives/\(drive.id)"); await changed(); dismiss() }
                        catch { self.error = error.localizedDescription }
                    }
                }
                Button("Cancel", role: .cancel) {}
            }
        }
        .onAppear { name = drive.name }
        #if os(macOS)
        .frame(width: 480, height: 460)
        #endif
    }
}

/// Connected volumes the server is not using yet. Nothing is used until it is added here.
private struct AddDriveView: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) private var dismiss
    let added: () async -> Void
    @State private var candidates: [DriveCandidate]?
    @State private var error: String?
    @State private var adding: String?
    var body: some View {
        NavigationStack {
            Form {
                SwiftUI.Section {
                    if let candidates {
                        if candidates.isEmpty { Text("Connect a drive to your server Mac to add it here.").foregroundStyle(Theme.muted) }
                        ForEach(candidates) { candidate in
                            HStack {
                                VStack(alignment: .leading, spacing: 3) {
                                    Text(verbatim: candidate.name)
                                    Text(L10n.format("%@ free of %@", bytes(candidate.freeBytes), bytes(candidate.totalBytes))).font(.caption).foregroundStyle(Theme.muted).monospacedDigit()
                                    if candidate.usedBefore { Text("Used by Torrbaby before").font(.caption).foregroundStyle(Theme.muted) }
                                    if !candidate.writable { Text("This drive is read-only").font(.caption).foregroundStyle(.orange) }
                                }
                                Spacer()
                                if adding == candidate.mount { ProgressView() } else {
                                    Button("Add") { add(candidate) }.disabled(adding != nil || !candidate.writable)
                                }
                            }
                        }
                    } else { HStack { Spacer(); ProgressView(); Spacer() } }
                } footer: { Text("Torrbaby makes a Media folder on the drive, with Downloads, Movies and TV inside, and keeps a small file there to recognise the drive.") }
                if let error { Text(L10n.text(error)).foregroundStyle(.orange) }
            }.formStyle(.grouped)
            .navigationTitle(L10n.text("Add Drive"))
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } } }
        }
        .task {
            do { let list: DriveCandidates = try await store.request("GET", "/v1/drives/candidates"); candidates = list.candidates }
            catch { self.error = error.localizedDescription; candidates = [] }
        }
        #if os(macOS)
        .frame(width: 480, height: 400)
        #endif
    }
    private func add(_ candidate: DriveCandidate) {
        adding = candidate.mount; error = nil
        Task {
            defer { adding = nil }
            do {
                let _: ServerDrive = try await store.request("POST", "/v1/drives", body: ["mount": candidate.mount], timeout: 30)
                await added(); dismiss()
            } catch { self.error = error.localizedDescription }
        }
    }
}

/// Manage Files → Move to Another Drive, for the whole title. Shown only with two or more drives.
struct DriveMoveSection: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    let assetIDs: [String]
    @State private var preview: DriveMovePreview?
    @State private var move: DriveMove?
    @State private var target: DriveMovePreview.Choice?
    @State private var error: String?
    private func name(_ id: String) -> String { preview?.choices.first { $0.id == id }?.name ?? id }
    var body: some View {
        Group {
            if let move {
                VStack(alignment: .leading, spacing: 8) {
                    Text("Move to Another Drive").font(.headline)
                    switch move.state {
                    case "done": Label(L10n.format("Moved to %@", name(move.drive)), systemImage: "checkmark.circle.fill").foregroundStyle(Theme.accent)
                    case "failed": Text(L10n.text(move.error ?? "The move did not finish; the original files are kept")).foregroundStyle(.orange)
                    default:
                        Text(L10n.format("Moving to %@ · %lld%%", name(move.drive), Int64(move.fraction * 100)))
                        ProgressView(value: move.fraction)
                        Text(LocalizedStringKey(move.waiting ?? "The server will continue this operation after you leave this screen.")).font(.caption).foregroundStyle(Theme.muted)
                    }
                }
            } else if let preview, !preview.choices.isEmpty {
                VStack(alignment: .leading, spacing: 10) {
                    Text("Move to Another Drive").font(.headline)
                    Text("Files are copied and checked before the originals are removed. Watch history and subtitles are kept.").font(.caption).foregroundStyle(Theme.muted)
                    ForEach(preview.choices) { choice in
                        Button { target = choice } label: {
                            HStack {
                                Image(systemName: choice.online ? "externaldrive" : "externaldrive.badge.xmark")
                                Text(verbatim: choice.name)
                                Spacer()
                                Text(verbatim: !choice.online ? L10n.text("Not connected") : choice.fits ? L10n.format("%@ free", bytes(choice.freeBytes)) : L10n.text("Not enough free space"))
                                    .font(.caption).foregroundStyle(choice.fits ? Theme.muted : .orange)
                            }.frame(maxWidth: .infinity).padding(10)
                        }.buttonStyle(MonoButtonStyle()).disabled(!choice.fits || store.busy)
                    }
                }
            }
            if let error { Text(L10n.text(error)).foregroundStyle(.orange) }
        }
        .confirmationDialog(L10n.format("Move to %@?", target?.name ?? ""), isPresented: Binding(get: { target != nil }, set: { if !$0 { target = nil } }), titleVisibility: .visible) {
            Button("Move") { if let target { start(target) } }
            Button("Cancel", role: .cancel) { target = nil }
        } message: { Text(L10n.format("%@ will be copied, checked, then removed from its current drive.", bytes(target?.bytes ?? 0))) }
        .task(id: assetIDs) {
            do {
                preview = try await store.request("POST", "/v1/drives/moves/preview", body: ["asset_ids": assetIDs])
                if preview?.moving == true { await poll() }
            } catch { if !Task.isCancelled { self.error = error.localizedDescription } }
        }
        .task(id: move?.id) { if move?.active == true { await poll() } }
    }
    private func current() async -> DriveMove? {
        guard let list: DriveMoves = try? await store.request("GET", "/v1/drives/moves") else { return nil }
        return list.moves.first { $0.id == move?.id } ?? list.moves.first { $0.active && $0.assetIds.contains(where: assetIDs.contains) }
    }
    private func poll() async {
        while !Task.isCancelled {
            if let latest = await current() {
                move = latest
                if !latest.active { await store.refresh(); return }
            }
            do { try await Task.sleep(for: .seconds(3)) } catch { return }
        }
    }
    private func start(_ choice: DriveMovePreview.Choice) {
        error = nil
        store.perform {
            let started: DriveMoveStarted = try await store.request("POST", "/v1/drives/moves", body: ["asset_ids": assetIDs, "drive": choice.id], timeout: 30)
            move = DriveMove(id: started.id, drive: choice.id, assetIds: assetIDs, state: "queued", bytes: choice.bytes, doneBytes: 0, error: nil, waiting: nil)
        }
    }
}
