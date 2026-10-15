// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import Foundation

enum InstallerCleanup {
    static func installed(_ app: URL, home: URL) -> Bool {
        let path = app.resolvingSymlinksInPath().path
        let folders = ["/Applications/", home.appendingPathComponent("Applications").path + "/"]
        return folders.contains { path.hasPrefix($0) }
    }

    static func installers(among volumes: [URL], holding bundleIdentifier: String, isInstaller: (URL) -> Bool) -> [URL] {
        volumes.filter { volume in
            guard isInstaller(volume) else { return false }
            let children = (try? FileManager.default.contentsOfDirectory(at: volume, includingPropertiesForKeys: nil)) ?? []
            return children.contains { $0.pathExtension == "app" && Bundle(url: $0)?.bundleIdentifier == bundleIdentifier }
        }
    }

    static func isReadOnlyEjectable(_ volume: URL) -> Bool {
        let values = try? volume.resourceValues(forKeys: [.volumeIsReadOnlyKey, .volumeIsEjectableKey])
        return values?.volumeIsReadOnly == true && values?.volumeIsEjectable == true
    }

    static func ejectInstallers(of bundle: Bundle = .main) {
        guard let id = bundle.bundleIdentifier,
              installed(bundle.bundleURL, home: FileManager.default.homeDirectoryForCurrentUser) else { return }
        let volumes = FileManager.default.mountedVolumeURLs(includingResourceValuesForKeys: [.volumeIsReadOnlyKey, .volumeIsEjectableKey],
                                                            options: [.skipHiddenVolumes]) ?? []
        for volume in installers(among: volumes, holding: id, isInstaller: isReadOnlyEjectable) {
            do {
                try NSWorkspace.shared.unmountAndEjectDevice(at: volume)
            } catch {
                NSLog("Cua Spaces: could not eject %@: %@", volume.path, error.localizedDescription)
            }
        }
    }
}
