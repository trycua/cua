// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpacesFFI
import Foundation

/// The app core's messages (`AppMsg`: a stable code plus variables, never
/// English) as text. The code is the key in `Localizable.xcstrings`, the same
/// key the web UI's `en` namespaces use; `{name}` in the text takes the
/// variable `name`. A code with no entry shows as the code.
public enum CoreMessages {
    public static func text(_ message: AppMsg) -> String {
        text(code: message.code, vars: message.vars)
    }

    public static func text(code: String, vars: [String: String] = [:]) -> String {
        let bundle = ModuleResources.bundle ?? .main
        var out = bundle.localizedString(forKey: code, value: code, table: nil)
        for (name, value) in vars { out = out.replacingOccurrences(of: "{\(name)}", with: value) }
        return out
    }
}
