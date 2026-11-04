// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpacesNotchHelper
import Foundation

// "Cua Spaces Notch": the notch for the Electron Cua Spaces app on macOS.
// The app launches it with pipes on stdin and stdout (the notch protocol,
// `NotchProtocol`); it is not meant to be opened by hand.
//
//   --selftest   decode sample messages without opening a window, then exit
//   --version    print the protocol version
let args = CommandLine.arguments.dropFirst()
if args.contains("--version") {
    print(NotchProtocol.version)
    exit(0)
}
if args.contains("--selftest") {
    exit(MainActor.assumeIsolated { NotchHelperRunner.selftest() })
}
MainActor.assumeIsolated { NotchHelperRunner.run() }
