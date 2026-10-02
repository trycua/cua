import Foundation
import CuaSpacesTranscript

// A scrubber you can drive from a shell.
//
//   swift run cast-render <cast> frame <seconds>   rendered screen at T
//   swift run cast-render <cast> json  <seconds>   JSON-UI document at T
//   swift run cast-render <cast> times             every instant the screen changed
//   swift run cast-render <cast> info              header, duration, CLI version
//
// This is tooling, not SDK surface: it exists so a human can look at a golden
// and judge it, which is the only thing that makes a golden trustworthy.

let arguments = CommandLine.arguments
guard arguments.count >= 3 else {
    FileHandle.standardError.write(Data("""
    usage: cast-render <cast> (frame|json|times|info) [seconds]

    """.utf8))
    exit(2)
}

let url = URL(fileURLWithPath: arguments[1])
let command = arguments[2]
let time = arguments.count > 3 ? (Double(arguments[3]) ?? 0) : Double.greatestFiniteMagnitude

do {
    let player = try CastPlayer(contentsOf: url)
    switch command {
    case "info":
        let header = player.recording.header
        print("cli:        \(header.cli ?? "unknown")")
        print("version:    \(header.cliVersion ?? "unknown")")
        print("script:     \(header.scriptName ?? "-")")
        print("size:       \(header.width)x\(header.height)")
        print("duration:   \(String(format: "%.3f", player.recording.duration))s")
        print("events:     \(player.recording.events.count)")
        print("scrubbed:   \(header.scrubbed.joined(separator: ", "))")
        if !header.notes.isEmpty { print("notes:      \(header.notes.joined(separator: " | "))") }
    case "times":
        for t in player.outputTimes { print(String(format: "%.3f", t)) }
    case "frame":
        let frame = player.frame(at: time)
        print("-- t=\(String(format: "%.3f", frame.effectiveTime)) "
              + "alt=\(frame.screen.isAlternateScreen) title=\(frame.windowTitle) "
              + "unsupported=\(frame.unsupportedSequences)")
        for (index, line) in frame.lines.enumerated() {
            print(String(format: "%2d|", index) + line)
        }
    case "json":
        let frame = player.frame(at: time)
        let document = ClaudeCodeParser().parse(frame: frame).jsonUIDocument()
        print(try document.prettyJSONString())
    default:
        FileHandle.standardError.write(Data("unknown command \(command)\n".utf8))
        exit(2)
    }
} catch {
    FileHandle.standardError.write(Data("error: \(error)\n".utf8))
    exit(1)
}
