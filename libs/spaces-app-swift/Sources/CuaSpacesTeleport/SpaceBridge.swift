// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Cua
import CuaSpacesFFI
import CuaSpaces

public extension SDKTeleportHost {
    /// A host for a `CuaSpaces.Space` backed by the cua SDK (nil for a Space
    /// on another transport).
    static func forSpace(_ space: CuaSpaces.Space, teleport: Teleport) async throws -> SDKTeleportHost? {
        guard let native = try await space.native() else { return nil }
        return SDKTeleportHost(teleport: teleport, space: native)
    }
}
