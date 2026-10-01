// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
import Foundation

extension AppModel {
    /// The name this app shows as on a Space's presence (its cursor's name
    /// pill for everyone else), decided by the app core as in the Tauri app:
    /// the signed-in account's name, else its email's local part, else this
    /// Mac's account (full, then short name). Never empty, an agent's or "You".
    var presenceName: String {
        Self.presenceName(profile: identity == nil ? nil : account?.profile(),
                          fullName: NSFullUserName(), user: NSUserName())
    }

    static func presenceName(profile: AccountProfile?, fullName: String, user: String) -> String {
        appPresenceName(name: profile?.name, email: profile?.email, username: profile?.username,
                        osFullName: fullName, osUser: user)
    }
}
