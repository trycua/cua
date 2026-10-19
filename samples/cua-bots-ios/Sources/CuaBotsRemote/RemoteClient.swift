// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Cua
import CoreGraphics
import CuaBotsCore
import CuaSpacesFFI
import Foundation

/// How the phone reaches a bot's computer.
public enum RemoteEndpoint: Equatable, Sendable {
    /// Through the Cua relay, as the signed-in account: the bot's Space
    /// published as a relay machine.
    case relay(relayURL: String?, accountToken: String, machineID: String)
    /// A spacesd address and env token (a LAN address, or a relay URL of the
    /// form `https://<relay>/m/<machine-id>`).
    case direct(url: String, token: String)

    /// `cuabots://direct?url=...&token=...` or
    /// `cuabots://relay?machine=...` (the token comes from sign-in).
    public static func parse(_ link: String, accountToken: String? = nil) -> RemoteEndpoint? {
        guard let comps = URLComponents(string: link), comps.scheme == "cuabots" else { return nil }
        let q = Dictionary((comps.queryItems ?? []).map { ($0.name, $0.value ?? "") }, uniquingKeysWith: { a, _ in a })
        switch comps.host {
        case "direct":
            guard let url = q["url"], !url.isEmpty, let token = q["token"], !token.isEmpty else { return nil }
            return .direct(url: url, token: token)
        case "relay":
            guard let machine = q["machine"], !machine.isEmpty, let token = accountToken ?? q["token"] else { return nil }
            return .relay(relayURL: q["relay"], accountToken: token, machineID: machine)
        default:
            return nil
        }
    }

    public var label: String {
        switch self {
        case .relay(_, _, let m): "Relay · \(m.prefix(8))"
        case .direct(let url, _): URL(string: url)?.host ?? url
        }
    }
}

/// The iOS app's connection to one bot: its snapshot, its outbox and its
/// screen, all through the bot Space's spacesd.
@MainActor
public final class RemoteBot: ObservableObject {
    @Published public private(set) var snapshot: RemoteSnapshot?
    @Published public private(set) var frame: CGImage?
    @Published public private(set) var frameSize: CGSize = .zero
    @Published public private(set) var error: String?
    @Published public private(set) var connected = false
    /// Commands sent but not yet reflected in a snapshot.
    @Published public private(set) var pending: [RemoteCommand] = []

    public let client: SpacesdClient
    public let home: String
    public let botID: String
    private var media: MediaSession?
    private var sink: FrameCollector?
    private var loop: Task<Void, Never>?

    public init(client: SpacesdClient, home: String, botID: String) {
        self.client = client
        self.home = home
        self.botID = botID
    }

    /// Connect and find the bot in that Space (its home under `~/bots/`).
    public static func connect(_ endpoint: RemoteEndpoint, cua: Cua) async throws -> [RemoteBot] {
        let client: SpacesdClient
        switch endpoint {
        case .relay(let url, let token, let machine):
            client = try await Relay(relayUrl: url, accountToken: token).connect(machineId: machine)
        case .direct(let url, let token):
            client = try await cua.spacesd(url: url, token: token)
        }
        return try await bots(on: client)
    }

    /// Every bot whose home is in this Space.
    public static func bots(on client: SpacesdClient) async throws -> [RemoteBot] {
        let out = try await client.sh(line: "ls -1d \"$HOME\"/bots/*/ 2>/dev/null", timeoutMs: 5000)
        let dirs = String(decoding: out.stdout, as: UTF8.self).split(separator: "\n").map {
            String($0).hasSuffix("/") ? String($0.dropLast()) : String($0)
        }
        return await MainActor.run {
            dirs.map { RemoteBot(client: client, home: $0, botID: ($0 as NSString).lastPathComponent) }
        }
    }

    // MARK: - State

    public func refresh() async {
        do {
            let data = try await client.download(path: "\(home)/\(RemoteSnapshot.path)")
            let snap = try RemoteSnapshot.decode(data)
            snapshot = snap
            // A sent message is done once the Mac shows it (clocks may differ
            // between the phone and the Mac, so match on the text too).
            pending.removeAll { c in
                if case .message(let t) = c.action {
                    return snap.messages.contains { $0.role == .user && $0.text == t } || c.sentAt.addingTimeInterval(120) < Date()
                }
                return c.sentAt <= snap.written
            }
            connected = true
            error = nil
        } catch {
            connected = snapshot != nil
            self.error = snapshot == nil ? "Waiting for the Mac to publish \(botID)" : nil
        }
    }

    /// Refresh every couple of seconds while the app is in front.
    public func startRefreshing(every interval: Duration = .seconds(2)) {
        loop?.cancel()
        loop = Task { [weak self] in
            while !Task.isCancelled {
                await self?.refresh()
                try? await Task.sleep(for: interval)
            }
        }
    }

    public func stopRefreshing() { loop?.cancel(); loop = nil }

    // MARK: - Commands

    public func send(_ action: RemoteCommand.Action) async {
        let command = RemoteCommand(botID: botID, action: action)
        do {
            let name = "\(Int(command.sentAt.timeIntervalSince1970 * 1000))-\(command.id).json"
            _ = try await client.upload(
                path: "\(home)/\(RemoteSnapshot.outbox)/\(name)", data: try command.encoded(),
                options: UploadOptions(createNew: true, append: false, permissions: 0o600, createParents: true))
            pending.append(command)
        } catch {
            self.error = "Couldn't reach \(snapshot?.bot.name ?? botID): \(error.localizedDescription)"
        }
    }

    /// The conversation with what the phone just sent shown right away.
    public var messages: [ChatMessage] {
        var list = snapshot?.messages ?? []
        for c in pending {
            if case .message(let text) = c.action {
                list.append(ChatMessage(botID: botID, role: .user, text: text, date: c.sentAt))
            }
        }
        return list
    }

    // MARK: - The computer

    /// Stream the bot's screen (decoded BGRA frames from spacesd's media
    /// plane). On the phone the computer opens in your control.
    public func startStream(maxDimension: UInt32 = 1280) async {
        guard media == nil else { return }
        let collector = FrameCollector { [weak self] image, size in
            Task { @MainActor in
                self?.frame = image
                self?.frameSize = size
            }
        }
        sink = collector
        do {
            media = try await client.openMediaDecoded(
                options: MediaOpenOptions(display: nil, windowHandle: nil, maxFps: 15, maxDimension: maxDimension,
                                          audio: false, disableVideo: false, requestJson: nil),
                frames: collector)
        } catch {
            self.error = "Couldn't open the computer: \(error.localizedDescription)"
        }
    }

    public func stopStream() async {
        try? await media?.close()
        media = nil
        sink = nil
    }

    /// A tap on the stream, in the frame's pixel coordinates.
    public func tap(at point: CGPoint) async {
        try? await client.click(x: point.x, y: point.y)
    }

    public func type(_ text: String) async {
        try? await client.typeText(text: text)
    }
}

/// Turns spacesd's decoded BGRA frames into images.
final class FrameCollector: DecodedFrameSink, @unchecked Sendable {
    let deliver: (CGImage, CGSize) -> Void
    init(_ deliver: @escaping (CGImage, CGSize) -> Void) { self.deliver = deliver }

    func onDecodedFrame(frame: DecodedVideoFrame) {
        guard let image = Self.image(frame) else { return }
        deliver(image, CGSize(width: Int(frame.width), height: Int(frame.height)))
    }

    func onEvent(event: MediaEvent) {}

    static func image(_ f: DecodedVideoFrame) -> CGImage? {
        imageFromBGRA(width: Int(f.width), height: Int(f.height), stride: Int(f.stride), data: f.data)
    }

    /// BGRA, premultiplied, little-endian 32-bit: what spacesd always sends.
    static func imageFromBGRA(width: Int, height: Int, stride: Int, data: Data) -> CGImage? {
        guard width > 0, height > 0, stride >= width * 4, data.count >= stride * height,
              let provider = CGDataProvider(data: data as CFData) else { return nil }
        return CGImage(width: width, height: height, bitsPerComponent: 8, bitsPerPixel: 32,
                       bytesPerRow: stride, space: CGColorSpaceCreateDeviceRGB(),
                       bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedFirst.rawValue
                                                | CGBitmapInfo.byteOrder32Little.rawValue),
                       provider: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent)
    }
}
