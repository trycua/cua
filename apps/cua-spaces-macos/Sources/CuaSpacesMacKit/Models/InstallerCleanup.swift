// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import Foundation

public protocol InstallerVolumes: AnyObject, Sendable {
    func attachedImages() -> Data?
    func appBundleIdentifiers(in mountPoint: String) -> [String]
    func eject(_ mountPoint: String) throws
    func trash(_ path: String) throws
}

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

    static let keptKey = "InstallerCleanup.kept"

    public init(volumes: InstallerVolumes, bundleIdentifier: String, bundlePath: String,
                home: String = NSHomeDirectory(), defaults: UserDefaults = .standard) {
        self.volumes = volumes
        self.bundleIdentifier = bundleIdentifier
        self.bundlePath = bundlePath
        self.home = home
        self.defaults = defaults
    }

    public static func forThisApp(_ bundle: Bundle = .main) -> InstallerCleanup? {
        guard bundle.bundleURL.pathExtension == "app", let id = bundle.bundleIdentifier,
              installed(bundle.bundlePath, home: NSHomeDirectory()) else { return nil }
        return InstallerCleanup(volumes: SystemInstallerVolumes(), bundleIdentifier: id,
                                bundlePath: bundle.bundlePath)
    }

    static func installed(_ bundlePath: String, home: String) -> Bool {
        bundlePath.hasPrefix("/Applications/") || bundlePath.hasPrefix(home + "/Applications/")
    }

    static func images(_ plist: Data) -> [InstallerImage] {
        guard let root = try? PropertyListSerialization.propertyList(from: plist, format: nil) as? [String: Any],
              let images = root["images"] as? [[String: Any]] else { return [] }
        return images.compactMap { image in
            guard let path = image["image-path"] as? String else { return nil }
            let entities = image["system-entities"] as? [[String: Any]] ?? []
            return InstallerImage(path: path, mountPoints: entities.compactMap { $0["mount-point"] as? String })
        }
    }

    func leftovers() -> [InstallerImage] {
        guard Self.installed(bundlePath, home: home), let plist = volumes.attachedImages() else { return [] }
        return Self.images(plist).filter { image in
            image.mountPoints.contains { volumes.appBundleIdentifiers(in: $0).contains(bundleIdentifier) }
        }
    }

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

    public func trash(_ image: InstallerImage) throws {
        try volumes.trash(image.path)
    }

    public func keep(_ image: InstallerImage) {
        let kept = defaults.stringArray(forKey: Self.keptKey) ?? []
        guard !kept.contains(image.path) else { return }
        defaults.set(kept + [image.path], forKey: Self.keptKey)
    }
}

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

public final class FixtureInstallerVolumes: InstallerVolumes, @unchecked Sendable {
    public var images: Data?
    public var apps: [String: [String]]
    public var ejectFailure: String?
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
