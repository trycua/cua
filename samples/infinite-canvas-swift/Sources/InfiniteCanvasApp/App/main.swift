// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit

// `infinite-canvas`: see Options.swift for flags and README.md for a tour.
let options: Options
do {
    options = try Options.parse(CommandLine.arguments)
} catch {
    FileHandle.standardError.write(Data("infinite-canvas: \(error)\n".utf8))
    exit(2)
}
MainActor.assumeIsolated {
    let app = NSApplication.shared
    app.setActivationPolicy(options.windowed == nil ? .accessory : .regular)
    let delegate = CanvasApp(options: options)
    app.delegate = delegate
    withExtendedLifetime(delegate) { app.run() }
}
