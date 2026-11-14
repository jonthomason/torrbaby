import SwiftUI
import CoreImage.CIFilterBuiltins
#if os(iOS)
import AVFoundation
import UIKit
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
    #if os(iOS)
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
                            #if os(iOS)
                            // A device far away (a TV that is never at home) shows its own code instead.
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
        #if os(iOS)
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
        CameraScanner { value in
            do {
                let decoded = try JSONDecoder().decode(PairingPayload.self, from: Data(value.utf8))
                guard decoded.app == "torrbaby", decoded.version == 1, decoded.code.count == 64,
                      let url = URL(string: decoded.server), ["http", "https"].contains(url.scheme ?? ""),
                      url.host != nil, url.user == nil, url.password == nil, url.query == nil, url.fragment == nil else {
                    throw APIError(message: "This isn’t a Torrbaby connection code.")
                }
                payload = decoded
            } catch { self.error = "This isn’t a valid Torrbaby connection code." }
        } failed: { error = $0 }
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

/// Let a new device join from anywhere (docs/remote-pairing.md): scan the code it shows, which
/// holds a one-time secret, and hand the secret to Diamond over this device's own connection.
/// The new device then fetches its setup through bronze, sealed with that secret.
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
                        Image(systemName: join.platform?.contains("tv") == true ? "tv" : "iphone").font(.largeTitle)
                        Text("Let this device join?").font(.title2.bold())
                        Text(verbatim: join.name ?? "").font(.callout)
                        Button(LocalizedStringKey(busy ? "Allowing…" : "Allow")) { Task { await allow(join) } }
                            .buttonStyle(MonoButtonStyle(prominent: true)).disabled(busy)
                        Button("Scan again") { self.join = nil; error = nil; scannerID = UUID() }.disabled(busy)
                        if let error { Text(L10n.text(error)).foregroundStyle(.orange) }
                    }.padding(24)
                } else {
                    VStack(spacing: 16) {
                        CameraScanner { value in
                            guard let decoded = try? JSONDecoder().decode(Join.self, from: Data(value.utf8)),
                                  decoded.app == "torrbaby", decoded.kind == "join", decoded.v == 1, decoded.secret.count == 43 else {
                                error = "This isn’t a Torrbaby setup code."; return
                            }
                            join = decoded
                        } failed: { error = $0 }
                        .id(scannerID)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                        .clipShape(RoundedRectangle(cornerRadius: 10))
                        Text("Scan the code shown on the new device").font(.headline)
                        if let error {
                            Text(L10n.text(error)).font(.caption).foregroundStyle(.orange)
                            Button("Try again") { self.error = nil; scannerID = UUID() }
                        }
                    }.padding(20)
                }
            }.navigationTitle("Scan the new device's code")
                .navigationBarTitleDisplayMode(.inline)
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
