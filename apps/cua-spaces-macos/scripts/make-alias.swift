// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

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
