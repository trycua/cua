import AppKit

final class SnapshotPublicationFixture: NSObject {
    let window: NSWindow
    private let directory: URL
    private let content = NSView(frame: NSRect(x: 0, y: 0, width: 720, height: 700))
    private var original: NSButton!
    private var replacement: NSButton?
    private var timer: Timer?
    private var generation = 0
    private var checkpoint = false
    private var originalClicks = 0
    private var replacementClicks = 0

    init(directory: URL) {
        self.directory = directory
        window = NSWindow(
            contentRect: NSRect(x: 100, y: 100, width: 720, height: 700),
            styleMask: [.titled, .closable], backing: .buffered, defer: false)
        super.init()
        window.title = "CuaTestHarness Snapshot Publication"
        window.isReleasedWhenClosed = false
        window.contentView = content
        let bitmap = NSBitmapImageRep(
            bitmapDataPlanes: nil, pixelsWide: 720, pixelsHigh: 600,
            bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
            colorSpaceName: .deviceRGB, bytesPerRow: 720 * 4, bitsPerPixel: 32)!
        let pixels = bitmap.bitmapData!
        var seed: UInt32 = 3473
        for index in 0..<(720 * 600) {
            seed ^= seed << 13
            seed ^= seed >> 17
            seed ^= seed << 5
            pixels[index * 4] = UInt8(truncatingIfNeeded: seed)
            pixels[index * 4 + 1] = UInt8(truncatingIfNeeded: seed >> 8)
            pixels[index * 4 + 2] = UInt8(truncatingIfNeeded: seed >> 16)
            pixels[index * 4 + 3] = 255
        }
        let image = NSImage(size: NSSize(width: 720, height: 600))
        image.addRepresentation(bitmap)
        let view = NSImageView(frame: NSRect(x: 0, y: 0, width: 720, height: 600))
        view.image = image
        view.setAccessibilityElement(false)
        content.addSubview(view)
        original = NSButton(title: "Original", target: self, action: #selector(clickOriginal))
        original.frame = NSRect(x: 20, y: 630, width: 200, height: 36)
        original.setAccessibilityIdentifier("snapshot-original")
        content.addSubview(original)
    }

    func show() {
        window.makeKeyAndOrderFront(nil)
        publish()
        timer = Timer.scheduledTimer(withTimeInterval: 0.02, repeats: true) { [weak self] _ in
            self?.readCommand()
        }
    }

    private func readCommand() {
        guard let command = try? String(contentsOf: directory.appendingPathComponent("command"), encoding: .utf8) else { return }
        if command == "checkpoint" && !checkpoint {
            checkpoint = true
            publish()
            return
        }
        guard generation == 0 && command == "replace" else { return }
        original.removeFromSuperview()
        let button = NSButton(title: "Replacement", target: self, action: #selector(clickReplacement))
        button.frame = original.frame
        button.setAccessibilityIdentifier("snapshot-replacement")
        content.addSubview(button)
        replacement = button
        generation = 1
        window.displayIfNeeded()
        publish()
    }

    @objc private func clickOriginal() {
        originalClicks += 1
        publish()
    }

    @objc private func clickReplacement() {
        replacementClicks += 1
        publish()
    }

    private func publish() {
        do {
            let data = try JSONSerialization.data(withJSONObject: [
                "pid": ProcessInfo.processInfo.processIdentifier,
                "window_id": window.windowNumber,
                "generation": generation,
                "checkpoint": checkpoint,
                "original_clicks": originalClicks,
                "replacement_clicks": replacementClicks,
            ])
            try data.write(to: directory.appendingPathComponent("state.json"), options: .atomic)
        } catch {
            fatalError("snapshot fixture state write failed: \(error)")
        }
    }
}
