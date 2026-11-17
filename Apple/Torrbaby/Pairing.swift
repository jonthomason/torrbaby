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
    @ViewBuilder private var buttons: some View {
        Button { use(QRPicture.fromClipboard()) } label: { Label("Paste picture", systemImage: "doc.on.clipboard").frame(maxWidth: .infinity) }
        PhotosPicker(selection: $photo, matching: .images) { Label("Photo", systemImage: "photo").frame(maxWidth: .infinity) }
        Button { choosingFile = true } label: { Label("File", systemImage: "folder").frame(maxWidth: .infinity) }
    }
    var body: some View {
        VStack(spacing: 10) { buttons }
        .buttonStyle(MonoButtonStyle())
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
    /// Opened as Settings → Link a device (rather than from a code screen): offers unpairing too.
    var linkPage = false
    @State private var join: Join?
    @State private var allowed = false
    @State private var error: String?
    @State private var busy = false
    @State private var scannerID = UUID()
    @State private var confirmingUnpair = false
    @State private var unpaired = false
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
                if unpaired {
                    // Unpaired: this device now shows its own code, for a paired one to allow.
                    ScrollView { OwnCodeView { dismiss() }.padding(24) }
                } else if allowed {
                    VStack(spacing: 18) {
                        Image(systemName: "checkmark.circle.fill").font(.system(size: 80)).foregroundStyle(Theme.accent)
                        Text("Device allowed").font(.title2.bold())
                        Button { dismiss() } label: { Text("Done").frame(maxWidth: .infinity) }.buttonStyle(MonoButtonStyle(prominent: true))
                    }.padding(24)
                } else if let join {
                    VStack(spacing: 18) {
                        Image(systemName: join.platform?.contains("tv") == true ? "tv" : join.platform == "macos" ? "laptopcomputer" : "iphone").font(.largeTitle)
                        Text("Let this device join?").font(.title2.bold())
                        Text(verbatim: join.name ?? "").font(.callout)
                        Button { Task { await allow(join) } } label: { Text(LocalizedStringKey(busy ? "Allowing…" : "Allow")).frame(maxWidth: .infinity) }
                            .buttonStyle(MonoButtonStyle(prominent: true)).disabled(busy)
                        Button { self.join = nil; error = nil; scannerID = UUID() } label: { Text("Scan again").frame(maxWidth: .infinity) }
                            .buttonStyle(MonoButtonStyle()).disabled(busy)
                        if let error { Text(L10n.text(error)).foregroundStyle(.orange) }
                    }.padding(24)
                } else {
                    VStack(spacing: 16) {
                        #if os(iOS)
                        CameraScanner { read($0) } failed: { error = $0 }
                        .id(scannerID)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                        .clipShape(RoundedRectangle(cornerRadius: 10))
                        #else
                        Image(systemName: "qrcode.viewfinder").font(.system(size: 48)).foregroundStyle(Theme.accent)
                        #endif
                        QRPictureSources(found: read, failed: { error = $0 })
                        if let error {
                            Text(L10n.text(error)).font(.caption).foregroundStyle(.orange)
                            #if os(iOS)
                            Button("Try again") { self.error = nil; scannerID = UUID() }
                            #endif
                        }
                        if linkPage {
                            Divider().padding(.vertical, 4)
                            Button(role: .destructive) { confirmingUnpair = true } label: { Text("Unpair this device").frame(maxWidth: .infinity) }
                                .buttonStyle(MonoButtonStyle()).foregroundStyle(.red)
                                .confirmationDialog("Unpair this device?", isPresented: $confirmingUnpair, titleVisibility: .visible) {
                                    Button("Unpair", role: .destructive) { Task { await unpair() } }
                                    Button("Cancel", role: .cancel) {}
                                }
                        }
                    }.padding(20)
                    #if os(macOS)
                    .frame(minWidth: 420, minHeight: 320)
                    #endif
                }
            }.navigationTitle(LocalizedStringKey(linkPage ? "Link a device" : "Scan the new device's code"))
                #if os(iOS)
                .navigationBarTitleDisplayMode(.inline)
                #endif
                .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() }.disabled(busy) } }
        }
    }
    /// Remove this device's own credential on Diamond and forget it here.
    private func unpair() async {
        busy = true; defer { busy = false }
        do { try await store.send("DELETE", "/v1/device-credentials/" + store.clientId) }
        catch { self.error = error.localizedDescription; return }
        try? Credentials.save("")
        store.token = ""
        unpaired = true
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

#if !os(tvOS)
/// Finds Diamond on the home network (Bonjour, _torrbaby._tcp), for a device not paired yet.
@MainActor
final class HomeServerFinder: NSObject, ObservableObject, @preconcurrency NetServiceBrowserDelegate, @preconcurrency NetServiceDelegate {
    @Published private(set) var addresses: [String] = []
    private let browser = NetServiceBrowser()
    private var services: [NetService] = []
    func start() { browser.delegate = self; browser.searchForServices(ofType: "_torrbaby._tcp.", inDomain: "local.") }
    func stop() { browser.stop(); services.forEach { $0.stop() }; services.removeAll() }
    func netServiceBrowser(_ browser: NetServiceBrowser, didFind service: NetService, moreComing: Bool) {
        services.append(service); service.delegate = self; service.resolve(withTimeout: 10)
    }
    func netServiceDidResolveAddress(_ sender: NetService) {
        guard let host = sender.hostName, sender.port > 0 else { return }
        var parts = URLComponents(); parts.scheme = "http"; parts.host = host.hasSuffix(".") ? String(host.dropLast()) : host; parts.port = sender.port
        if let value = parts.url?.absoluteString, !addresses.contains(value) { addresses.append(value) }
    }
}

/// This device's own setup code, shown while it is not paired (docs/remote-pairing.md,
/// version 1), as an Android TV shows one: a connected device reads it (camera, or a pasted
/// or photographed picture of it) and allows it; this device then claims its setup through
/// Torrbaby's relay on bronze, sealed with a key only the code holds. The code stays the same
/// until it is used: nothing happens until a connected device allows it.
struct OwnCodeView: View {
    @ObservedObject private var interfaceLanguage = InterfaceLanguage.shared
    @EnvironmentObject var store: Store
    let connected: () -> Void
    /// Built in for a device that has never been paired; the setup replaces them with Diamond's.
    static let relayAddress = "47.112.18.69:7443"
    static let relayFingerprint = "c0fcde9603eec131f8d4a7ace67e7aad2900e5f9b6a59c6818106b0a51d6d7d6"
    @State private var code: CGImage?
    @State private var status = ""
    @State private var copied = false
    @StateObject private var home = HomeServerFinder()
    var body: some View {
        VStack(spacing: 16) {
            if let code {
                let image = Image(decorative: code, scale: 1).interpolation(.none)
                image.resizable().scaledToFit().padding(16).background(.white).frame(maxWidth: 300, maxHeight: 300)
                    .accessibilityLabel("Setup code")
                VStack(spacing: 10) {
                    // To paste into WeChat, Messages or any other app.
                    Button { copy(code); copied = true } label: {
                        Label(LocalizedStringKey(copied ? "Copied" : "Copy code"), systemImage: copied ? "checkmark" : "doc.on.doc").frame(maxWidth: .infinity)
                    }
                    ShareLink(item: image, preview: SharePreview("Setup code", image: image)) {
                        Label("Send", systemImage: "square.and.arrow.up").frame(maxWidth: .infinity)
                    }
                }.buttonStyle(MonoButtonStyle()).frame(maxWidth: 360)
            } else { ProgressView().frame(height: 300) }
            if !status.isEmpty { Text(L10n.text(status)).font(.callout).foregroundStyle(.secondary).multilineTextAlignment(.center) }
        }.frame(maxWidth: 520).frame(maxWidth: .infinity)
        .onAppear { home.start() }
        .onDisappear { home.stop() }
        .task { await run() }
    }
    private func copy(_ code: CGImage) {
        #if os(iOS)
        UIPasteboard.general.image = UIImage(cgImage: code)
        #else
        NSPasteboard.general.clearContents()
        NSPasteboard.general.writeObjects([NSImage(cgImage: code, size: NSSize(width: code.width, height: code.height))])
        #endif
    }
    /// The secret is kept until it is used, so a code sent as a picture stays good.
    private static func secret() -> Data {
        if let kept = Data(base64Encoded: Credentials.read(account: "join-secret")), kept.count == 32 { return kept }
        let fresh = RemotePairing().secret
        try? Credentials.save(fresh.base64EncodedString(), account: "join-secret")
        return fresh
    }
    private func run() async {
        let pairing = RemotePairing(secret: Self.secret())
        let payload = ["app": "torrbaby", "kind": "join", "v": 1, "secret": pairing.secretText, "name": store.defaultClientName, "platform": store.clientPlatform] as [String: Any]
        guard let json = try? JSONSerialization.data(withJSONObject: payload) else { return }
        code = Self.qr(json)
        if store.relayAddress.isEmpty || store.relayFingerprint.count != 64 { store.learnRelay(address: Self.relayAddress, fingerprint: Self.relayFingerprint) }
        let started = Date()
        while !Task.isCancelled {
            // At home Diamond is asked directly; away (iPhone, iPad), through the relay on bronze.
            // Nothing is pinned yet on a new device: the claim goes plain, and its reply is sealed.
            var routes = home.addresses
            #if os(iOS)
            if let link = await store.ensureLink() { routes.append(link) }
            #endif
            var reached = false
            for route in routes {
                guard let url = URL(string: route + "/v1/pairings/remote/claim") else { continue }
                var request = URLRequest(url: url); request.httpMethod = "POST"; request.timeoutInterval = 10
                request.setValue("application/json", forHTTPHeaderField: "Content-Type")
                request.httpBody = try? JSONSerialization.data(withJSONObject: ["id": pairing.id])
                guard let (data, _) = try? await URLSession.shared.data(for: request) else { continue }
                reached = true
                if let setup = pairing.open(data) { await finish(setup, at: route); return }
            }
            status = reached || Date().timeIntervalSince(started) < 15 ? "" : "Can’t reach your server yet. Still trying…"
            // Every 2 s while someone is likely reading it, then every 5 s.
            try? await Task.sleep(for: .seconds(Date().timeIntervalSince(started) < 600 ? 2 : 5))
        }
    }
    private func finish(_ setup: RemotePairing.Setup, at route: String) async {
        status = "Setting up…"
        guard let identity = setup.server?.identity, identity.count == 64 else {
            status = "Your server did not send its identity. Update Torrbaby on the server, then choose New setup code."; return
        }
        store.learnRelay(address: setup.relay?.address, fingerprint: setup.relay?.fingerprint)
        store.learnIdentity(identity, from: "from pairing")
        do {
            try Credentials.save(setup.token)
            try Credentials.save(setup.client_id, account: "client-id")
        } catch { status = error.localizedDescription; return }
        try? Credentials.save("", account: "join-secret")
        store.clientId = setup.client_id
        // Kept by its home address (the one it was found at, or Diamond's own); away, the relay carries it.
        let base = home.addresses.contains(route) ? route : "http://\(setup.routes?.home ?? "diamond.local"):8765"
        store.server = base; store.token = setup.token
        UserDefaults.standard.set(base, forKey: "server")
        await store.connect()
        connected()
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
