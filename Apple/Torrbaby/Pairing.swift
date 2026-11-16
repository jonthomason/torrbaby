import SwiftUI
import CoreImage.CIFilterBuiltins
import CryptoKit
import UniformTypeIdentifiers
#if !os(tvOS)
import PhotosUI
#endif
#if os(iOS)
import AVFoundation
import UIKit
#elseif os(macOS)
import AppKit
#endif

/// A QR code read from a picture: pasted (a code received in WeChat or as a photo, copied),
/// chosen from Photos, or from a file.
enum QRPicture {
    static func read(_ image: CIImage) -> String? {
        let detector = CIDetector(ofType: CIDetectorTypeQRCode, context: nil, options: [CIDetectorAccuracy: CIDetectorAccuracyHigh])
        return detector?.features(in: image).compactMap { ($0 as? CIQRCodeFeature)?.messageString }.first
    }
    static func read(data: Data) -> String? { CIImage(data: data).flatMap(read) }
    /// The code in the picture on the clipboard (or in a copied image file); nil when there is none.
    static func fromClipboard() -> String? {
        #if os(iOS)
        let board = UIPasteboard.general
        if let image = board.image, let found = (image.cgImage.map { CIImage(cgImage: $0) } ?? CIImage(image: image)).flatMap(read) { return found }
        if let data = board.data(forPasteboardType: UTType.image.identifier) { return read(data: data) }
        return nil
        #elseif os(macOS)
        let board = NSPasteboard.general
        if let urls = board.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL] {
            for url in urls { if let data = try? Data(contentsOf: url), let found = read(data: data) { return found } }
        }
        if let images = board.readObjects(forClasses: [NSImage.self]) as? [NSImage] {
            for image in images {
                if let cg = image.cgImage(forProposedRect: nil, context: nil, hints: nil), let found = read(CIImage(cgImage: cg)) { return found }
                if let tiff = image.tiffRepresentation, let found = read(data: tiff) { return found }
            }
        }
        return nil
        #else
        return nil
        #endif
    }
}

#if !os(tvOS)
/// Paste a picture of a code, or choose one from Photos or a file: beside the camera on
/// iPhone and iPad, and the way to read a code on the Mac.
struct QRPictureSources: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    let found: (String) -> Void
    let failed: (String) -> Void
    @State private var photo: PhotosPickerItem?
    @State private var choosingFile = false
    private func use(_ value: String?) {
        if let value { found(value) } else { failed("No QR code was found in that picture.") }
    }
    var body: some View {
        HStack(spacing: 12) {
            Button { use(QRPicture.fromClipboard()) } label: { Label("Paste picture", systemImage: "doc.on.clipboard") }
            PhotosPicker(selection: $photo, matching: .images) { Label("Photo", systemImage: "photo") }
            Button { choosingFile = true } label: { Label("File", systemImage: "folder") }
        }
        .buttonStyle(MonoButtonStyle(compact: true))
        .onChange(of: photo) { _, item in
            guard let item else { return }
            Task {
                let data = try? await item.loadTransferable(type: Data.self)
                photo = nil
                use(data.flatMap(QRPicture.read(data:)))
            }
        }
        .fileImporter(isPresented: $choosingFile, allowedContentTypes: [.image]) { result in
            guard case .success(let url) = result else { return }
            let scoped = url.startAccessingSecurityScopedResource()
            defer { if scoped { url.stopAccessingSecurityScopedResource() } }
            use((try? Data(contentsOf: url)).flatMap(QRPicture.read(data:)))
        }
    }
}
#endif

private struct PairingPayload: Codable {
    let app: String
    let version: Int
    let server: String
    let code: String
    var fallbackServer: String? = nil
    /// Torrbaby's rendezvous on bronze ("ip:port") and its certificate's SHA-256: a device
    /// with neither the home network nor Tailscale pairs through the relay.
    var relay: String? = nil
    var relayFingerprint: String? = nil
    /// Diamond's identity fingerprint (end-to-end encryption through bronze), from a device
    /// that learned it at home.
    var identity: String? = nil
}

struct PairingCodeView: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) private var dismiss
    @State private var grant: Grant?
    @State private var error: String?
    @State private var paired = false
    @State private var generation = UUID()
    #if !os(tvOS)
    @State private var showJoin = false
    #endif
    private struct Grant: Decodable { let id: String; let code: String; let expires: Double }
    private struct Status: Decodable { let paired: Bool }
    private var qr: CGImage? {
        guard let grant, let data = try? JSONEncoder().encode(PairingPayload(app: "torrbaby", version: 1, server: store.server, code: grant.code, fallbackServer: store.fallbackServer.isEmpty ? nil : store.fallbackServer, relay: store.relayAddress.isEmpty ? nil : store.relayAddress, relayFingerprint: store.relayFingerprint.isEmpty ? nil : store.relayFingerprint, identity: store.diamondIdentity.isEmpty ? nil : store.diamondIdentity)) else { return nil }
        let filter = CIFilter.qrCodeGenerator()
        filter.message = data
        filter.correctionLevel = "M"
        guard let output = filter.outputImage?.transformed(by: CGAffineTransform(scaleX: 8, y: 8)) else { return nil }
        return CIContext().createCGImage(output, from: output.extent)
    }
    var body: some View {
        NavigationStack {
            VStack(spacing: 18) {
                Text(LocalizedStringKey(paired ? "Device linked" : "Link a device")).font(.title.bold())
                if paired {
                    Image(systemName: "checkmark.circle.fill").font(.system(size: 80)).foregroundStyle(Theme.accent)
                    Button("Done") { dismiss() }.buttonStyle(MonoButtonStyle(prominent: true))
                } else if let error {
                    Text(L10n.text(error)).foregroundStyle(.orange)
                    Button("New code") { generation = UUID() }
                } else if let grant, let qr {
                    TimelineView(.periodic(from: .now, by: 1)) { context in
                        if context.date.timeIntervalSince1970 < grant.expires {
                            Image(decorative: qr, scale: 1).interpolation(.none).resizable().scaledToFit()
                                .padding(16).background(.white).frame(width: 280, height: 280)
                            Text("Open Torrbaby on your iPhone or iPad and choose Scan connection code.")
                                .multilineTextAlignment(.center)
                            Text("Expires in \(Int(grant.expires - context.date.timeIntervalSince1970)) seconds")
                                .font(.caption.monospacedDigit()).foregroundStyle(.secondary)
                            #if !os(tvOS)
                            // A device far away shows its own code instead: read it with the
                            // camera, or from a picture of it (pasted, a photo, a file).
                            Button("Scan the new device's code") { showJoin = true }
                            #endif
                        } else {
                            Text("This code has expired.")
                            Button("New code") { generation = UUID() }
                        }
                    }
                } else { ProgressView() }
            }.padding(30).frame(maxWidth: 420, minHeight: 410)
                .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Close") { dismiss() } } }
        }
        #if !os(tvOS)
        .sheet(isPresented: $showJoin) { LocalizedInterface { JoinScannerView() }.environmentObject(store) }
        #endif
        .task(id: generation) {
            error = nil; paired = false
            if let grant { try? await store.send("DELETE", "/v1/pairings/" + grant.id) }
            grant = nil
            do {
                let created: Grant = try await store.request("POST", "/v1/pairings")
                grant = created
                while !Task.isCancelled && Date().timeIntervalSince1970 < created.expires {
                    try await Task.sleep(for: .seconds(2))
                    let status: Status = try await store.request("GET", "/v1/pairings/" + created.id)
                    if status.paired { paired = true; await store.refresh(); break }
                }
            } catch { if !Task.isCancelled { self.error = error.localizedDescription } }
        }
        .onDisappear {
            if let grant { Task { try? await store.send("DELETE", "/v1/pairings/" + grant.id) } }
        }
    }
}

#if os(iOS)
struct PairingScannerView: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) private var dismiss
    let connected: () -> Void
    @State private var payload: PairingPayload?
    @State private var error: String?
    @State private var busy = false
    @State private var scannerID = UUID()
    var body: some View {
        NavigationStack {
            Group {
                if let payload {
                    ScrollView {
                        VStack(spacing: 18) {
                            Image(systemName: "network").font(.largeTitle)
                            Text("Connect to this server?").font(.title2.bold())
                            Text(payload.server).font(.callout).textSelection(.enabled)
                            Button(LocalizedStringKey(busy ? "Connecting…" : "Connect")) { Task { await redeem(payload) } }
                                .buttonStyle(MonoButtonStyle(prominent: true)).disabled(busy)
                            Button("Scan again") { self.payload = nil; error = nil; scannerID = UUID() }.disabled(busy)
                            if let error { Text(L10n.text(error)).foregroundStyle(.orange) }
                        }.frame(maxWidth: .infinity).padding(24)
                    }
                } else {
                    GeometryReader { geometry in
                        let landscape = geometry.size.width > geometry.size.height
                        let layout = landscape ? AnyLayout(HStackLayout(spacing: 20)) : AnyLayout(VStackLayout(spacing: 20))
                        layout {
                            camera.id(scannerID)
                                .frame(maxWidth: .infinity, maxHeight: .infinity)
                                .clipShape(RoundedRectangle(cornerRadius: 10))
                            VStack(spacing: 12) {
                                Image(systemName: "qrcode.viewfinder").font(.system(size: 40)).foregroundStyle(Theme.accent)
                                Text("Scan the QR code").font(.headline)
                                Text("Or use a picture of it: one sent to you in WeChat or as a photo.").font(.caption).foregroundStyle(.secondary)
                                QRPictureSources(found: read, failed: { error = $0 })
                                if let error {
                                    Text(L10n.text(error)).font(.caption).foregroundStyle(.orange)
                                    Button("Try again") { self.error = nil; scannerID = UUID() }
                                }
                            }.multilineTextAlignment(.center)
                                .frame(maxWidth: landscape ? 190 : .infinity)
                        }.padding(20)
                    }
                }
            }.navigationTitle("Scan connection code")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() }.disabled(busy) } }
        }
    }
    private var camera: some View {
        CameraScanner { read($0) } failed: { error = $0 }
    }
    private func read(_ value: String) {
            do {
                let decoded = try JSONDecoder().decode(PairingPayload.self, from: Data(value.utf8))
                guard decoded.app == "torrbaby", decoded.version == 1, decoded.code.count == 64,
                      let url = URL(string: decoded.server), ["http", "https"].contains(url.scheme ?? ""),
                      url.host != nil, url.user == nil, url.password == nil, url.query == nil, url.fragment == nil else {
                    throw APIError(message: "This isn’t a Torrbaby connection code.")
                }
                error = nil; payload = decoded
            } catch { self.error = "This isn’t a valid Torrbaby connection code." }
    }
    private func redeem(_ payload: PairingPayload) async {
        busy = true; error = nil
        defer { busy = false }
        do {
            // The code names the server's home route and its Tailscale route; a device pairing
            // away from home reaches it by the second, or else through the relay on bronze.
            var routes = [payload.server, payload.fallbackServer ?? ""].map { $0.trimmingCharacters(in: CharacterSet(charactersIn: "/")) }.filter { !$0.isEmpty }
            store.learnRelay(address: payload.relay, fingerprint: payload.relayFingerprint)
            store.learnIdentity(payload.identity, from: "from pairing")
            if payload.relay != nil, let relay = await store.ensureRelay() {
                // With the relay, Tailscale is not used: home first, then the relay.
                routes = Array(routes.prefix(1)) + [relay]
            }
            var base = routes.first ?? ""
            var reply: (Data, URLResponse)?
            for route in routes {
                guard let url = URL(string: route + "/v1/pairings/redeem") else { continue }
                var request = URLRequest(url: url)
                request.httpMethod = "POST"
                request.timeoutInterval = route == routes.last ? 20 : 6
                request.setValue("application/json", forHTTPHeaderField: "Content-Type")
                request.httpBody = try JSONSerialization.data(withJSONObject: ["code":payload.code,"name":store.defaultClientName,"platform":store.clientPlatform])
                // Through the relay, the server is still kept by its home address.
                do { reply = try await URLSession.shared.data(for: request); base = route == store.relayRoute ? (routes.first ?? route) : route; break }
                catch let failure as URLError where failure.code != .cancelled && route != routes.last { continue }
            }
            guard let (data, response) = reply else { throw APIError(message: "Invalid server address") }
            guard let http = response as? HTTPURLResponse, http.statusCode == 200 else {
                throw APIError(message: "The code expired or was already used. Show a new code on the linked device.")
            }
            struct Redeemed: Decodable { let client_id: String; let token: String }
            let result = try JSONDecoder().decode(Redeemed.self, from: data)
            try Credentials.save(result.token)
            try Credentials.save(result.client_id, account: "client-id")
            store.clientId = result.client_id
            store.server = base; store.token = result.token
            if let fallback = payload.fallbackServer { try store.saveFallback(fallback) }
            UserDefaults.standard.set(base, forKey: "server")
            await store.connect()
            guard store.connected else { throw APIError(message: store.connectionError ?? "Could not connect") }
            dismiss(); connected()
        } catch { self.error = error.localizedDescription }
    }
}

#endif

#if !os(tvOS)
/// Let a new device join from anywhere (docs/remote-pairing.md): read the code it shows (with
/// the camera, or from a pasted, photographed or saved picture of it), which holds a one-time
/// secret, and hand the secret to Diamond over this device's own connection. The new device
/// then fetches its setup through bronze, sealed with that secret.
struct JoinScannerView: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) private var dismiss
    struct Join: Decodable { let app: String; let kind: String; let v: Int; let secret: String; let name: String?; let platform: String? }
    @State private var join: Join?
    @State private var allowed = false
    @State private var error: String?
    @State private var busy = false
    @State private var scannerID = UUID()
    private func read(_ value: String) {
        guard let decoded = try? JSONDecoder().decode(Join.self, from: Data(value.utf8)),
              decoded.app == "torrbaby", decoded.kind == "join", decoded.v == 1, decoded.secret.count == 43 else {
            error = "This isn’t a Torrbaby setup code."; return
        }
        error = nil; join = decoded
    }
    var body: some View {
        NavigationStack {
            Group {
                if allowed {
                    VStack(spacing: 18) {
                        Image(systemName: "checkmark.circle.fill").font(.system(size: 80)).foregroundStyle(Theme.accent)
                        Text("Device allowed").font(.title2.bold())
                        Text("It finishes setting up by itself within a few seconds, wherever it is.").multilineTextAlignment(.center)
                        Button("Done") { dismiss() }.buttonStyle(MonoButtonStyle(prominent: true))
                    }.padding(24)
                } else if let join {
                    VStack(spacing: 18) {
                        Image(systemName: join.platform?.contains("tv") == true ? "tv" : join.platform == "macos" ? "laptopcomputer" : "iphone").font(.largeTitle)
                        Text("Let this device join?").font(.title2.bold())
                        Text(verbatim: join.name ?? "").font(.callout)
                        Button(LocalizedStringKey(busy ? "Allowing…" : "Allow")) { Task { await allow(join) } }
                            .buttonStyle(MonoButtonStyle(prominent: true)).disabled(busy)
                        Button("Scan again") { self.join = nil; error = nil; scannerID = UUID() }.disabled(busy)
                        if let error { Text(L10n.text(error)).foregroundStyle(.orange) }
                    }.padding(24)
                } else {
                    VStack(spacing: 16) {
                        #if os(iOS)
                        CameraScanner { read($0) } failed: { error = $0 }
                        .id(scannerID)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                        .clipShape(RoundedRectangle(cornerRadius: 10))
                        Text("Scan the code shown on the new device").font(.headline)
                        Text("Or use a picture of it: one sent to you in WeChat or as a photo.").font(.caption).foregroundStyle(.secondary).multilineTextAlignment(.center)
                        #else
                        Image(systemName: "qrcode.viewfinder").font(.system(size: 48)).foregroundStyle(Theme.accent)
                        Text("Paste or choose a picture of the code shown on the new device").font(.headline).multilineTextAlignment(.center)
                        Text("Copy the picture from WeChat, Messages or Photos, then choose Paste picture.").font(.callout).foregroundStyle(.secondary).multilineTextAlignment(.center)
                        #endif
                        QRPictureSources(found: read, failed: { error = $0 })
                        if let error {
                            Text(L10n.text(error)).font(.caption).foregroundStyle(.orange)
                            #if os(iOS)
                            Button("Try again") { self.error = nil; scannerID = UUID() }
                            #endif
                        }
                    }.padding(20)
                    #if os(macOS)
                    .frame(minWidth: 420, minHeight: 320)
                    #endif
                }
            }.navigationTitle("Scan the new device's code")
                #if os(iOS)
                .navigationBarTitleDisplayMode(.inline)
                #endif
                .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() }.disabled(busy) } }
        }
    }
    private func allow(_ join: Join) async {
        busy = true; defer { busy = false }
        do {
            struct Granted: Decodable { let id: String }
            let _: Granted = try await store.request("POST", "/v1/pairings/remote", body: ["secret": join.secret, "name": join.name ?? "", "platform": join.platform ?? ""])
            allowed = true
        } catch { self.error = error.localizedDescription }
    }
}
#endif

#if os(iOS)
/// A new iPhone or iPad set up from anywhere, as an Android TV is (docs/remote-pairing.md,
/// version 1): it shows a code with a fresh secret; a connected device reads it and hands the
/// secret to Diamond; this device then claims its setup through Torrbaby's relay on bronze,
/// sealed with a key only the code holds, so bronze can neither read nor forge it.
struct JoinAwayView: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) private var dismiss
    let connected: () -> Void
    /// Built in for a device that has never been paired; the setup replaces them with Diamond's.
    static let relayAddress = "47.112.18.69:7443"
    static let relayFingerprint = "c0fcde9603eec131f8d4a7ace67e7aad2900e5f9b6a59c6818106b0a51d6d7d6"
    @State private var generation = UUID()
    @State private var code: CGImage?
    @State private var status = "Connecting to Torrbaby’s relay…"
    @State private var expired = false
    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: 18) {
                    Text("Scan this code with Torrbaby on a phone or tablet that is already connected to your server.")
                        .multilineTextAlignment(.center)
                    if let code {
                        let image = Image(decorative: code, scale: 1).interpolation(.none)
                        image.resizable().scaledToFit().padding(16).background(.white).frame(maxWidth: 300, maxHeight: 300)
                            .accessibilityLabel("Setup code")
                        // Sent as a picture (WeChat, Messages) to someone with a connected device.
                        ShareLink(item: image, preview: SharePreview("Setup code", image: image)) {
                            Label("Send this code", systemImage: "square.and.arrow.up")
                        }.buttonStyle(MonoButtonStyle())
                    } else { ProgressView().frame(height: 300) }
                    Text("No connected phone nearby? Take a photo of this code and send it to someone who has one. It works once and expires in 10 minutes.")
                        .font(.callout).foregroundStyle(.secondary).multilineTextAlignment(.center)
                    Text(L10n.text(status)).font(.callout).foregroundStyle(expired ? .orange : .secondary).multilineTextAlignment(.center)
                    Button("New setup code") { generation = UUID() }.buttonStyle(MonoButtonStyle())
                }.padding(24).frame(maxWidth: 520).frame(maxWidth: .infinity)
            }
            .navigationTitle("Set up away from home").navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } } }
        }
        .task(id: generation) { await run() }
    }
    private func run() async {
        expired = false; code = nil; status = "Connecting to Torrbaby’s relay…"
        let pairing = RemotePairing()
        let payload = ["app": "torrbaby", "kind": "join", "v": 1, "secret": pairing.secretText, "name": store.defaultClientName, "platform": store.clientPlatform] as [String: Any]
        guard let json = try? JSONSerialization.data(withJSONObject: payload) else { return }
        code = Self.qr(json)
        if store.relayAddress.isEmpty || store.relayFingerprint.count != 64 { store.learnRelay(address: Self.relayAddress, fingerprint: Self.relayFingerprint) }
        let deadline = Date().addingTimeInterval(10 * 60)
        var said = false
        while !Task.isCancelled && Date() < deadline {
            // Nothing pinned yet on a new device: the claim goes plain through bronze, and its reply is sealed.
            if let relay = await store.ensureRelay(), let url = URL(string: relay + "/v1/pairings/remote/claim") {
                if !said { status = "Waiting for the code to be scanned…"; said = true }
                var request = URLRequest(url: url); request.httpMethod = "POST"; request.timeoutInterval = 15
                request.setValue("application/json", forHTTPHeaderField: "Content-Type")
                request.httpBody = try? JSONSerialization.data(withJSONObject: ["id": pairing.id])
                do {
                    let (data, _) = try await URLSession.shared.data(for: request)
                    if let setup = pairing.open(data) { await finish(setup); return }
                } catch { status = "Your server is not answering through the relay yet. Still trying…"; said = false }
            } else { status = "Could not reach Torrbaby’s relay. Check this device’s internet connection; still trying…" }
            try? await Task.sleep(for: .seconds(2))
        }
        if !Task.isCancelled { expired = true; status = "This code has expired. Choose New setup code to show another." }
    }
    private func finish(_ setup: RemotePairing.Setup) async {
        status = "Setting up…"
        guard let identity = setup.server?.identity, identity.count == 64 else {
            status = "Your server did not send its identity. Update Torrbaby on the server, then choose New setup code."; expired = true; return
        }
        store.learnRelay(address: setup.relay?.address, fingerprint: setup.relay?.fingerprint)
        store.learnIdentity(identity, from: "from pairing")
        do {
            try Credentials.save(setup.token)
            try Credentials.save(setup.client_id, account: "client-id")
        } catch { status = error.localizedDescription; expired = true; return }
        store.clientId = setup.client_id
        // Kept by its home address, as a device paired through the relay is; away, the relay carries it.
        let base = "http://\(setup.routes?.home ?? "diamond.local"):8765"
        store.server = base; store.token = setup.token
        UserDefaults.standard.set(base, forKey: "server")
        await store.connect()
        dismiss(); connected()
    }
    private static func qr(_ data: Data) -> CGImage? {
        let filter = CIFilter.qrCodeGenerator()
        filter.message = data; filter.correctionLevel = "M"
        guard let output = filter.outputImage?.transformed(by: CGAffineTransform(scaleX: 8, y: 8)) else { return nil }
        return CIContext().createCGImage(output, from: output.extent)
    }
}
#endif

/// The new device's half of remote pairing (docs/remote-pairing.md, version 1).
struct RemotePairing {
    struct Setup: Decodable {
        struct Server: Decodable { let id: String?; let name: String?; let identity: String? }
        struct Relay: Decodable { let address: String?; let fingerprint: String? }
        struct Routes: Decodable { let home: String?; let away: String? }
        let v: Int; let client_id: String; let token: String
        let server: Server?; let relay: Relay?; let routes: Routes?
    }
    let secret: Data
    let id: String
    let key: SymmetricKey
    init(secret: Data = Data((0..<32).map { _ in UInt8.random(in: 0...255) })) {
        self.secret = secret
        let digest = SHA256.hash(data: Data("torrbaby-pair-id-v1".utf8) + secret)
        id = String(digest.map { String(format: "%02x", $0) }.joined().prefix(32))
        key = HKDF<SHA256>.deriveKey(inputKeyMaterial: SymmetricKey(data: secret), salt: Data("torrbaby-pair-v1".utf8), info: Data("seal".utf8), outputByteCount: 32)
    }
    var secretText: String { secret.base64EncodedString().replacingOccurrences(of: "+", with: "-").replacingOccurrences(of: "/", with: "_").replacingOccurrences(of: "=", with: "") }
    /// The setup from a claim reply; nil while not granted, or when the reply does not open (not
    /// sealed by Diamond for this code: ignored, as the protocol says).
    func open(_ reply: Data) -> Setup? {
        guard let object = try? JSONSerialization.jsonObject(with: reply) as? [String: Any], var sealed = object["sealed"] as? String else { return nil }
        sealed = sealed.replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
        while sealed.count % 4 != 0 { sealed += "=" }
        guard let data = Data(base64Encoded: sealed), data.count >= 28,
              let box = try? AES.GCM.SealedBox(combined: data),
              let plain = try? AES.GCM.open(box, using: key, authenticating: Data(id.utf8)),
              let setup = try? JSONDecoder().decode(Setup.self, from: plain),
              setup.v == 1, !setup.token.isEmpty, !setup.client_id.isEmpty else { return nil }
        return setup
    }
}

#if os(iOS)
private struct CameraScanner: UIViewControllerRepresentable {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    let scanned: (String) -> Void
    let failed: (String) -> Void
    final class Controller: UIViewController, AVCaptureMetadataOutputObjectsDelegate {
        let capture = AVCaptureSession()
        let queue = DispatchQueue(label: "org.torrbaby.qr-camera")
        var scanned: (String) -> Void = { _ in }
        var failed: (String) -> Void = { _ in }
        var preview: AVCaptureVideoPreviewLayer?
        var rotationCoordinator: AVCaptureDevice.RotationCoordinator?
        var rotationObservation: NSKeyValueObservation?
        var delivered = false
        var dismissed = false
        override func viewDidLoad() {
            super.viewDidLoad()
            AVCaptureDevice.requestAccess(for: .video) { [weak self] allowed in
                DispatchQueue.main.async {
                    guard let self, !self.dismissed else { return }
                    if allowed { self.configure() }
                    else { self.failed("Allow Camera access in Settings to scan a connection code.") }
                }
            }
        }
        func configure() {
            guard !dismissed else { return }
            do {
                guard let camera = AVCaptureDevice.default(for: .video) else { throw APIError(message: "No camera is available on this device.") }
                let input = try AVCaptureDeviceInput(device: camera)
                let output = AVCaptureMetadataOutput()
                guard capture.canAddInput(input), capture.canAddOutput(output) else { throw APIError(message: "The camera could not start.") }
                capture.addInput(input); capture.addOutput(output)
                output.setMetadataObjectsDelegate(self, queue: .main)
                output.metadataObjectTypes = [.qr]
                let preview = AVCaptureVideoPreviewLayer(session: capture)
                preview.videoGravity = .resizeAspectFill
                view.layer.addSublayer(preview); self.preview = preview
                preview.frame = view.bounds
                let coordinator = AVCaptureDevice.RotationCoordinator(device: camera, previewLayer: preview)
                rotationCoordinator = coordinator
                rotationObservation = coordinator.observe(\.videoRotationAngleForHorizonLevelPreview, options: [.initial, .new]) { [weak self] _, _ in
                    DispatchQueue.main.async { self?.updateRotation() }
                }
                updateRotation()
                queue.async { [capture] in capture.startRunning() }
            } catch { failed(error.localizedDescription) }
        }
        private func updateRotation() {
            guard !dismissed, let angle = rotationCoordinator?.videoRotationAngleForHorizonLevelPreview,
                  let connection = preview?.connection,
                  connection.isVideoRotationAngleSupported(angle) else { return }
            connection.videoRotationAngle = angle
        }
        override func viewDidLayoutSubviews() {
            super.viewDidLayoutSubviews()
            CATransaction.begin()
            CATransaction.setDisableActions(true)
            preview?.frame = view.bounds
            updateRotation()
            CATransaction.commit()
        }
        func metadataOutput(_ output: AVCaptureMetadataOutput, didOutput metadataObjects: [AVMetadataObject], from connection: AVCaptureConnection) {
            guard !delivered, let value = (metadataObjects.first as? AVMetadataMachineReadableCodeObject)?.stringValue else { return }
            delivered = true
            queue.async { [capture] in capture.stopRunning() }
            scanned(value)
        }
    }
    func makeUIViewController(context: Context) -> Controller {
        let controller = Controller(); controller.scanned = scanned; controller.failed = failed
        return controller
    }
    func updateUIViewController(_ controller: Controller, context: Context) {}
    static func dismantleUIViewController(_ controller: Controller, coordinator: ()) {
        controller.dismissed = true
        controller.rotationObservation = nil
        controller.rotationCoordinator = nil
        controller.queue.async { [capture = controller.capture] in capture.stopRunning() }
    }
}
#endif
