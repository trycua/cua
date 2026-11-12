// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSpacesNotchUI
import Foundation

/// The helper process: an agent app (no Dock icon, no menu bar) that reads
/// the notch protocol from stdin, draws the notch and writes to stdout. It
/// exits when stdin closes, on `quit`, or when stdout is gone.
@MainActor
public enum NotchHelperRunner {
    /// Runs until Electron goes away.
    public static func run() -> Never {
        // A closed stdout must not kill us with SIGPIPE before we exit cleanly.
        signal(SIGPIPE, SIG_IGN)
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)
        let notch = HelperNotch(emit: write)
        let splitter = Splitter()
        let input = FileHandle.standardInput
        // The handler runs on one background queue at a time.
        input.readabilityHandler = { handle in
            let chunk = handle.availableData
            if chunk.isEmpty {
                // EOF: the app quit or crashed.
                handle.readabilityHandler = nil
                DispatchQueue.main.async { exit(0) }
                return
            }
            let lines = splitter.push(chunk)
            guard !lines.isEmpty else { return }
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    for line in lines {
                        do {
                            if !notch.receive(try HostMessage.decode(line)) {
                                exit(notch.greeted ? 0 : NotchProtocol.mismatchExit)
                            }
                        } catch {
                            log("bad message: \(error)")
                        }
                    }
                }
            }
        }
        app.run()
        exit(0)
    }

    /// The stdin reader's line splitter (only its handler touches it).
    private final class Splitter: @unchecked Sendable {
        private var lines = LineSplitter()
        func push(_ chunk: Data) -> [Data] { lines.push(chunk) }
    }

    /// One message to Electron; exits when it can no longer be written.
    static func write(_ message: HelperMessage) {
        do {
            try FileHandle.standardOutput.write(contentsOf: message.line())
        } catch {
            exit(0)
        }
    }

    static func log(_ text: String) {
        FileHandle.standardError.write(Data("cua-spaces-notch: \(text)\n".utf8))
    }

    /// `--selftest`: decodes sample messages and drives a notch without a
    /// panel (no window opens). Returns the exit status.
    public static func selftest() -> Int32 {
        var out: [HelperMessage] = []
        let notch = HelperNotch(emit: { out.append($0) })
        notch.makesPanel = false
        var splitter = LineSplitter()
        let lines = splitter.push(Data(sample.utf8))
        func fail(_ why: String) -> Int32 {
            log("selftest failed: \(why)")
            return 1
        }
        guard lines.count == 4 else { return fail("expected 4 sample lines, got \(lines.count)") }
        for line in lines {
            let message: HostMessage
            do { message = try HostMessage.decode(line) } catch { return fail("decode: \(error)") }
            guard notch.receive(message) else { break }
        }
        guard notch.greeted, case .hello(let v, _)? = out.first, v == NotchProtocol.version else {
            return fail("no hello back")
        }
        guard notch.view.phase == .tiles, notch.view.tiles.map(\.id) == ["local:aurora"] else {
            return fail("view not applied")
        }
        guard notch.geometry.stage == CGSize(width: 680, height: 260), notch.thumbnail("local:aurora") != nil else {
            return fail("layout or thumbnail not applied")
        }
        guard notch.panelUp else { return fail("no panel for a shown notch") }
        // A screen the core has no layout for still gets the panel (the
        // SwiftUI app keeps its panel before its first layout too).
        let bare = HelperNotch(emit: { _ in })
        bare.makesPanel = false
        for line in lines.prefix(2) {
            guard var message = try? HostMessage.decode(line) else { return fail("decode") }
            if case .state(var s) = message {
                s.layout = nil
                message = .state(s)
            }
            bare.receive(message)
        }
        guard bare.panelUp else { return fail("no panel without a layout") }
        notch.openSpace("local:aurora")
        let encoded = String(decoding: out.last?.line() ?? Data(), as: UTF8.self)
        guard encoded == #"{"action":"openSpace","spaceId":"local:aurora","type":"action"}"# + "\n" else {
            return fail("encoded \(encoded)")
        }
        print("cua-spaces-notch selftest ok (protocol \(NotchProtocol.version))")
        return 0
    }

    /// hello, a state with one tile and a layout, a 1x1 PNG thumbnail, quit.
    static let sample = """
    {"type":"hello","v":1,"motion":{"hoverDwellMs":300,"closeDelayMs":400,"openResponse":0.42,"openDamping":0.8,"closeResponse":0.45,"closeDamping":1,"reducedDuration":0.2,"hoverResponse":0.26,"hoverDamping":0.65,"hoverScale":1.08,"hoverScaleY":1.12,"contentDelayMs":90,"contentIn":0.22,"contentOut":0.12,"contentScale":0.96},"radii":{"closed":{"top":6,"bottom":14},"open":{"top":19,"bottom":24}}}
    {"type":"state","view":{"phase":"tiles","tiles":[{"id":"local:aurora","name":"aurora","status":"running","dim":false,"dropTarget":true,"targeted":false,"symbol":"apple","label":"aurora, running","location":"This Mac","signedIn":false}],"dropMode":false,"label":"Cua Spaces","countLabel":"1 Space","tab":{"count":"1","word":"Space"},"hidden":false,"showTab":true,"hoverCue":false},"query":"","layout":{"hasNotch":true,"notch":{"x":660,"y":950,"width":192,"height":32},"closedFrame":{"x":640,"y":940,"width":232,"height":42},"openFrame":{"x":436,"y":782,"width":640,"height":200},"promptFrame":{"x":641,"y":922,"width":230,"height":60},"tabFrame":{"x":852,"y":950,"width":44,"height":32},"tabInsetNotch":0,"tabInsetOuter":2,"stageFrame":{"x":416,"y":722,"width":680,"height":260},"notchStyle":true},"shown":true,"dragging":false,"icons":{"apple":{"symbol":"apple.logo"}}}
    {"type":"thumbnail","id":"local:aurora","image":"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=="}
    {"type":"quit"}

    """
}
