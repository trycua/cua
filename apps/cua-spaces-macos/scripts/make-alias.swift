// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Writes a Finder alias to a folder and gives it a custom icon. package-dmg.sh
// uses it for the disk image's Applications link: a symlink cannot carry its
// own icon, an alias file can (Finder resolves it by path on any Mac).
//
//   swift scripts/make-alias.swift /Applications "<stage>/Applications" Support/dmg/applications.icns
import AppKit

let args = CommandLine.arguments
guard args.count == 4 else {
    FileHandle.standardError.write("usage: make-alias.swift TARGET ALIAS ICON\n".data(using: .utf8)!)
    exit(2)
}
let target = URL(fileURLWithPath: args[1], isDirectory: true)
let alias = URL(fileURLWithPath: args[2])
do {
    let bookmark = try target.bookmarkData(options: .suitableForBookmarkFile, includingResourceValuesForKeys: nil, relativeTo: nil)
    try URL.writeBookmarkData(bookmark, to: alias)
} catch {
    FileHandle.standardError.write("could not write the alias: \(error)\n".data(using: .utf8)!)
    exit(1)
}
guard let icon = NSImage(contentsOfFile: args[3]),
      NSWorkspace.shared.setIcon(icon, forFile: alias.path, options: []) else {
    FileHandle.standardError.write("could not set the alias icon from \(args[3])\n".data(using: .utf8)!)
    exit(1)
}
