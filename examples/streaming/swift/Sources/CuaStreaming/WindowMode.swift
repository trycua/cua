import AppKit
import AVFoundation
import Cua
import Foundation

/// Window mode: an AppKit window showing the decoded frames and speaker
/// playback through AVAudioEngine. The scenario runs on a background task and
/// terminates the app when it is done.
enum WindowMode {
    static func run(cfg: Config) {
        let app = NSApplication.shared
        app.setActivationPolicy(.regular)
        let presenter = WindowPresenter()
        let delegate = AppDelegate(presenter: presenter)
        app.delegate = delegate
        Task.detached {
            var code: Int32 = 1
            do { code = try await CuaStreamingMain.runScenario(cfg: cfg, presenter: presenter) } catch {
                fputs("error: \(error)\n", stderr)
            }
            presenter.stopAudio()
            DispatchQueue.main.async { exit(code) }
        }
        withExtendedLifetime(delegate) { app.run() }
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    let presenter: WindowPresenter
    init(presenter: WindowPresenter) { self.presenter = presenter }

    func applicationDidFinishLaunching(_ notification: Notification) {
        presenter.makeWindow()
        NSApp.activate(ignoringOtherApps: true)
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
}

/// Renders BGRA frames into a layer and plays PCM. Frame callbacks arrive
/// on the SDK's delivery thread: at most one frame is in flight to the main
/// thread (newer frames replace a pending one), audio is scheduled directly.
final class WindowPresenter: StreamPresenter, @unchecked Sendable {
    private var window: NSWindow?
    private var view: NSView?
    private let lock = NSLock()
    private var pending: CGImage?
    private var scheduled = false

    private let engine = AVAudioEngine()
    private let player = AVAudioPlayerNode()
    private var format: AVAudioFormat?
    private let audioQueue = DispatchQueue(label: "cua.streaming.audio")

    func makeWindow() {
        let w = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 1024, height: 640),
            styleMask: [.titled, .closable, .resizable, .miniaturizable],
            backing: .buffered, defer: false)
        w.title = "cua streaming (Swift)"
        let v = NSView()
        v.wantsLayer = true
        v.layer?.contentsGravity = .resizeAspect
        v.layer?.backgroundColor = NSColor.black.cgColor
        w.contentView = v
        w.center()
        w.makeKeyAndOrderFront(nil)
        window = w
        view = v
    }

    func present(frame: DecodedVideoFrame) {
        guard let image = Self.image(frame) else { return }
        lock.lock()
        pending = image
        let needSchedule = !scheduled
        scheduled = true
        lock.unlock()
        guard needSchedule else { return }
        DispatchQueue.main.async { [self] in
            lock.lock()
            let img = pending
            pending = nil
            scheduled = false
            lock.unlock()
            view?.layer?.contents = img
        }
    }

    func play(audio: PcmAudio) {
        audioQueue.async { [self] in
            let ch = AVAudioChannelCount(max(1, audio.channels))
            if format == nil || format!.sampleRate != Double(audio.sampleRate) || format!.channelCount != ch {
                reconfigure(sampleRate: Double(audio.sampleRate), channels: ch)
            }
            guard let format else { return }
            let n = audio.samples.count / Int(ch)
            guard n > 0, let buf = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(n)),
                  let planes = buf.floatChannelData
            else { return }
            buf.frameLength = AVAudioFrameCount(n)
            for i in 0..<n {
                for c in 0..<Int(ch) { planes[c][i] = Float(audio.samples[i * Int(ch) + c]) / 32768 }
            }
            player.scheduleBuffer(buf)
        }
    }

    func stopAudio() {
        audioQueue.sync {
            player.stop()
            engine.stop()
        }
    }

    private func reconfigure(sampleRate: Double, channels: AVAudioChannelCount) {
        player.stop()
        engine.stop()
        if player.engine == nil { engine.attach(player) }
        guard let f = AVAudioFormat(
            commonFormat: .pcmFormatFloat32, sampleRate: sampleRate, channels: channels, interleaved: false)
        else { return }
        engine.connect(player, to: engine.mainMixerNode, format: f)
        do {
            try engine.start()
            player.play()
            format = f
        } catch {
            fputs("audio: \(error)\n", stderr)
            format = nil
        }
    }

    private static func image(_ f: DecodedVideoFrame) -> CGImage? {
        guard let provider = CGDataProvider(data: f.data as CFData) else { return nil }
        return CGImage(
            width: Int(f.width), height: Int(f.height), bitsPerComponent: 8, bitsPerPixel: 32,
            bytesPerRow: Int(f.stride), space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.noneSkipFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue),
            provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent)
    }
}
