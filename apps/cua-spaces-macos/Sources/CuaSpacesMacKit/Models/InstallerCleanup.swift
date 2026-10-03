// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import Foundation

/// The disk image side of a drag install. Finder copies the app out of the
/// image and leaves the image mounted and its .dmg in Downloads; macOS never
/// tidies up either, so the app does on launch: it ejects every mounted image
/// that holds Cua Spaces itself, then offers to move a .dmg in Downloads to
/// the Trash. Ejecting needs no permission. Touching Downloads makes macOS
/// ask for access to the folder, so the Trash waits for the user's yes and
/// that prompt follows a choice they made. Everything that reads the
/// system, ejects or trashes goes through this protocol, so tests run on
/// `FixtureInstallerVolumes` and never touch the Mac's disks.
public protocol InstallerVolumes: AnyObject, Sendable {
    /// `hdiutil info -plist`: the attached disk images.
    func attachedImages() -> Data?
    /// The bundle identifiers of the apps at the top of a mounted volume.
    func appBundleIdentifiers(in mountPoint: String) -> [String]
    /// Ejects a mounted volume.
    func eject(_ mountPoint: String) throws
    /// Moves a file to the Trash.
    func trash(_ path: String) throws
}

/// An attached disk image: its file and where its volumes are mounted.
public struct InstallerImage: Equatable, Sendable {
    public let path: String
    public let mountPoints: [String]

    public var name: String { (path as NSString).lastPathComponent }
}

public struct InstallerCleanup: @unchecked Sendable {
    let volumes: InstallerVolumes
    let bundleIdentifier: String
    let bundlePath: String
    let home: String
    let defaults: UserDefaults

    /// The .dmg paths the user chose to keep; they are not offered again.
    static let keptKey = "InstallerCleanup.kept"

    public init(volumes: InstallerVolumes, bundleIdentifier: String, bundlePath: String,
                home: String = NSHomeDirectory(), defaults: UserDefaults = .standard) {
        self.volumes = volumes
        self.bundleIdentifier = bundleIdentifier
        self.bundlePath = bundlePath
        self.home = home
        self.defaults = defaults
    }

    /// This running app on the real system; nil for a development build
    /// (no bundle identifier) or when it runs from anywhere but an
    /// Applications folder: from the image itself or a translocated copy of
    /// it, which must stay mounted.
    public static func forThisApp(_ bundle: Bundle = .main) -> InstallerCleanup? {
        guard bundle.bundleURL.pathExtension == "app", let id = bundle.bundleIdentifier,
              installed(bundle.bundlePath, home: NSHomeDirectory()) else { return nil }
        return InstallerCleanup(volumes: SystemInstallerVolumes(), bundleIdentifier: id,
                                bundlePath: bundle.bundlePath)
    }

    /// Whether the app runs from a place that outlives its installer: an
    /// Applications folder (as `stable_app_location` in cua-spaces-app-core),
    /// never a mounted image or App Translocation's copy of one.
    static func installed(_ bundlePath: String, home: String) -> Bool {
        bundlePath.hasPrefix("/Applications/") || bundlePath.hasPrefix(home + "/Applications/")
    }

    /// The attached images in `hdiutil info -plist`, with their mount points.
    static func images(_ plist: Data) -> [InstallerImage] {
        guard let root = try? PropertyListSerialization.propertyList(from: plist, format: nil) as? [String: Any],
              let images = root["images"] as? [[String: Any]] else { return [] }
        return images.compactMap { image in
            guard let path = image["image-path"] as? String else { return nil }
            let entities = image["system-entities"] as? [[String: Any]] ?? []
            return InstallerImage(path: path, mountPoints: entities.compactMap { $0["mount-point"] as? String })
        }
    }

    /// The images this app was installed from: mounted and holding an app
    /// with this bundle identifier. None unless this copy is installed, so
    /// the image it runs from is never among them.
    func leftovers() -> [InstallerImage] {
        guard Self.installed(bundlePath, home: home), let plist = volumes.attachedImages() else { return [] }
        return Self.images(plist).filter { image in
            image.mountPoints.contains { volumes.appBundleIdentifiers(in: $0).contains(bundleIdentifier) }
        }
    }

    /// Ejects every image this app was installed from. Returns the ejected
    /// ones whose .dmg is in Downloads and that the user has not kept: the
    /// ones to offer for the Trash.
    public func ejectLeftovers() -> [InstallerImage] {
        let kept = Set(defaults.stringArray(forKey: Self.keptKey) ?? [])
        let downloads = home + "/Downloads"
        return leftovers().filter { image in
            for mountPoint in image.mountPoints {
                do {
                    try volumes.eject(mountPoint)
                } catch {
                    NSLog("Cua Spaces: could not eject %@: %@", mountPoint, error.localizedDescription)
                    return false
                }
            }
            return (image.path as NSString).deletingLastPathComponent == downloads
                && image.path.hasSuffix(".dmg") && !kept.contains(image.path)
        }
    }

    /// Moves an ejected image's .dmg to the Trash.
    public func trash(_ image: InstallerImage) throws {
        try volumes.trash(image.path)
    }

    /// Remembers that the user keeps this .dmg, so it is not offered again.
    public func keep(_ image: InstallerImage) {
        let kept = defaults.stringArray(forKey: Self.keptKey) ?? []
        guard !kept.contains(image.path) else { return }
        defaults.set(kept + [image.path], forKey: Self.keptKey)
    }
}

/// The real system: `hdiutil`, the volumes' Info.plists, NSWorkspace and
/// the Trash.
public final class SystemInstallerVolumes: InstallerVolumes, @unchecked Sendable {
    public init() {}

    public func attachedImages() -> Data? {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/hdiutil")
        process.arguments = ["info", "-plist"]
        let out = Pipe()
        process.standardOutput = out
        process.standardError = FileHandle.nullDevice
        do { try process.run() } catch { return nil }
        let data = out.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        return process.terminationStatus == 0 ? data : nil
    }

    public func appBundleIdentifiers(in mountPoint: String) -> [String] {
        let names = (try? FileManager.default.contentsOfDirectory(atPath: mountPoint)) ?? []
        return names.filter { $0.hasSuffix(".app") }.compactMap { name in
            let plist = NSDictionary(contentsOfFile: "\(mountPoint)/\(name)/Contents/Info.plist")
            return plist?["CFBundleIdentifier"] as? String
        }
    }

    public func eject(_ mountPoint: String) throws {
        try NSWorkspace.shared.unmountAndEjectDevice(at: URL(fileURLWithPath: mountPoint))
    }

    public func trash(_ path: String) throws {
        try FileManager.default.trashItem(at: URL(fileURLWithPath: path), resultingItemURL: nil)
    }
}

/// In-memory disk images (tests, fixtures). `images` is what `hdiutil info
/// -plist` would print; `apps` maps mount points to the bundle identifiers
/// on them.
public final class FixtureInstallerVolumes: InstallerVolumes, @unchecked Sendable {
    public var images: Data?
    public var apps: [String: [String]]
    public var ejectFailure: String?
    /// `eject <mount point>` and `trash <path>`, in order.
    public private(set) var calls: [String] = []

    public init(images: Data?, apps: [String: [String]] = [:]) {
        self.images = images
        self.apps = apps
    }

    public func attachedImages() -> Data? { images }
    public func appBundleIdentifiers(in mountPoint: String) -> [String] { apps[mountPoint] ?? [] }

    public func eject(_ mountPoint: String) throws {
        calls.append("eject \(mountPoint)")
        if let ejectFailure { throw CocoaError(.fileWriteUnknown, userInfo: [NSLocalizedDescriptionKey: ejectFailure]) }
    }

    public func trash(_ path: String) throws { calls.append("trash \(path)") }
}
