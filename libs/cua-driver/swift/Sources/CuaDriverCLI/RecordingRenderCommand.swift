import ArgumentParser
import CuaDriverCore
import Foundation

/// `cua-driver recording render <input-dir> --output <out.mp4>` — post-process
/// a recording directory produced by `recording start … --video-experimental`
/// into a zoomed output MP4.
///
/// Unlike the `start`/`stop`/`status` subcommands, this one does NOT go
/// through the daemon — it's pure file-to-file work with no shared
/// in-process state, so the simplest path is to run it in the CLI
/// process directly. That also means `recording render` works fine
/// without a running `cua-driver serve`.
struct RecordingRenderCommand: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "render",
        abstract: "Render a captured recording directory to a zoomed MP4.",
        discussion: """
            Reads `<input-dir>/session.json`, `<input-dir>/recording.mp4`,
            `<input-dir>/cursor.jsonl`, and every `<input-dir>/turn-*/action.json`,
            then produces a single zoom-on-click MP4 at `--output`.

            The `--no-zoom` flag re-encodes the input without any zoom
            effects — useful for verifying the reader/writer pipeline
            independent of the zoom math.

            Examples:
              cua-driver recording render ~/cua-rec --output /tmp/out.mp4
              cua-driver recording render ~/cua-rec --output /tmp/baseline.mp4 --no-zoom
              cua-driver recording render ~/cua-rec --output /tmp/out.mp4 --scale 2.5
            """
    )

    @Argument(
        help: "Recording directory (contains session.json + recording.mp4 + cursor.jsonl + turn-*/)."
    )
    var inputDir: String

    @Option(
        name: .long,
        help: "Destination path for the rendered MP4. Overwrites any existing file."
    )
    var output: String

    @Flag(
        name: .customLong("no-zoom"),
        help: "Skip the zoom curve and re-encode the input as-is. Useful as a baseline check."
    )
    var noZoom: Bool = false

    @Option(
        name: .long,
        help: ArgumentHelp(
            "Zoom factor applied to each click event. 1.0 disables zoom; 2.0 is 2× magnification.",
            valueName: "factor"
        )
    )
    var scale: Double = 2.0

    func run() async throws {
        let expandedInput = (inputDir as NSString).expandingTildeInPath
        let inputURL = URL(fileURLWithPath: expandedInput).standardizedFileURL

        let expandedOutput = (output as NSString).expandingTildeInPath
        let outputURL = URL(fileURLWithPath: expandedOutput).standardizedFileURL

        // Fast-fail on obvious mistakes before spinning up AVFoundation.
        var isDir: ObjCBool = false
        guard FileManager.default.fileExists(atPath: inputURL.path, isDirectory: &isDir),
              isDir.boolValue else {
            printErrorLine("input directory not found: \(inputURL.path)")
            throw ExitCode(1)
        }

        // Ensure the output's parent exists — AVAssetWriter bubbles a less
        // helpful error otherwise.
        let parent = outputURL.deletingLastPathComponent()
        if !FileManager.default.fileExists(atPath: parent.path) {
            do {
                try FileManager.default.createDirectory(
                    at: parent, withIntermediateDirectories: true
                )
            } catch {
                printErrorLine(
                    "failed to create output parent \(parent.path): \(error.localizedDescription)"
                )
                throw ExitCode(1)
            }
        }

        let options = RecordingRenderer.Options(
            noZoom: noZoom,
            defaultScale: scale,
            progressIntervalMs: 1000
        )

        printErrorLine("rendering \(inputURL.path) → \(outputURL.path) "
            + "(no-zoom=\(noZoom), scale=\(scale))")

        let progress: RecordingRendererProgress = { frameIndex, tMs in
            let seconds = tMs / 1000.0
            let formatted = String(format: "%.1f", seconds)
            let line = "rendered frame \(frameIndex) (\(formatted)s)\n"
            FileHandle.standardError.write(Data(line.utf8))
        }

        let appended: Int
        do {
            appended = try await RecordingRenderer.render(
                from: inputURL,
                to: outputURL,
                options: options,
                progress: progress
            )
        } catch {
            printErrorLine("render failed: \(error)")
            throw ExitCode(1)
        }

        print("Rendered \(appended) frame\(appended == 1 ? "" : "s") -> \(outputURL.path)")
    }

    private func printErrorLine(_ text: String) {
        FileHandle.standardError.write(Data((text + "\n").utf8))
    }
}
