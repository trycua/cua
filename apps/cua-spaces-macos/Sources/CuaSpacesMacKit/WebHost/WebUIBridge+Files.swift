// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSpacesFFI
import Foundation
import UniformTypeIdentifiers

/// A Space's drop well in the page, as `SpaceDetailView`'s `TeleportDropZone`
/// does it: "Send file…" picks files natively, files dropped on the well are
/// read from the drag's own pasteboard (the page sees only their names),
/// and either list is sent with the app's transfer (`backend.sendFiles`).
/// An app bundle the page gets back opens Teleport at that app
/// (`teleport.entryForPath`). Only paths picked or dropped here are sent.
extension WebUIBridge {
    func routeFiles(_ method: String, _ args: [String: Any]) async throws -> Any? {
        switch method {
        case "spaces.chooseFiles":
            return offer(pickFiles() ?? [])
        case "windows.chooseIso":
            guard let image = args["image"] as? String else { throw Failure.badArgs("image: string") }
            // Cancelled: nothing chosen. Otherwise the file is checked and
            // remembered for the image at once, so a wrong file is told now.
            guard let url = pickIso() else { return NSNull() }
            let choice = useWindowsIso(image, url.path)
            guard choice.ok else { throw Failure(code: "bad_iso", message: choice.problem) }
            return ["name": choice.name]
        case "spaces.droppedFiles":
            guard let names = args["names"] as? [String] else { throw Failure.badArgs("names: [string]") }
            return offer(dropped(names: names))
        case "spaces.sendFiles":
            let id = try spaceId(args)
            guard let paths = args["paths"] as? [String], !paths.isEmpty else { throw Failure.badArgs("paths: [string]") }
            guard paths.allSatisfy(offeredPaths.contains) else {
                throw Failure(code: "forbidden", message: "only files picked or dropped here are sent")
            }
            guard model.spaces.contains(where: { $0.id == id }) else { throw Failure.notFound("no Space \(id)") }
            model.recordFeature("file_send")
            return BridgeValue.encode(try await model.backend.sendFiles(id: id, paths: paths))
        default:
            return nil
        }
    }

    /// The apps and files of the last drop on a notch tile whose names the
    /// page asks for, else the files on the last drag's pasteboard whose
    /// names the page saw dropped (an older drag's files never match).
    func dropped(names: [String]) -> [URL] {
        let wanted = Set(names)
        guard !wanted.isEmpty else { return [] }
        // A drop on a notch tile, handed over once.
        let fromNotch = WebUIWindowController.notchDrop.filter { wanted.contains($0.lastPathComponent) }
        if !fromNotch.isEmpty {
            WebUIWindowController.notchDrop = []
            return fromNotch
        }
        let urls = dragPasteboard().readObjects(forClasses: [NSURL.self],
                                                options: [.urlReadingFileURLsOnly: true]) as? [URL] ?? []
        return urls.filter { wanted.contains($0.lastPathComponent) }
    }

    /// Remembers `urls` as sendable and answers their paths.
    private func offer(_ urls: [URL]) -> [String] {
        let paths = urls.filter(\.isFileURL).map(\.path)
        offeredPaths.formUnion(paths)
        return paths
    }

    /// The picker for a Windows disc image (`.iso`).
    static func isoPanel() -> URL? {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = false
        panel.message = "Choose the Windows ISO you downloaded from Microsoft"
        panel.prompt = "Use This ISO"
        if let iso = UTType(filenameExtension: "iso") { panel.allowedContentTypes = [iso] }
        return panel.runModal() == .OK ? panel.url : nil
    }

    /// The native file picker, as `SpaceDetailView.chooseFiles`.
    static func openPanel() -> [URL]? {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = true
        panel.canChooseDirectories = true
        return panel.runModal() == .OK ? panel.urls : nil
    }
}
