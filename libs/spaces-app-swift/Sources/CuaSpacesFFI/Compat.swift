// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The spellings the Spaces apps used when the app export lived in the SDK:
// `cua.teleport()` and `host.setupRequest(...)` (UniFFI cannot add methods
// to another crate's objects, so the export has free functions).

import CuaSDK

public extension Cua {
    /// Teleport send: move app sessions from this machine into sandboxes.
    func teleport() -> Teleport {
        CuaSpacesFFI.teleport(cua: self)
    }
}

public extension Host {
    /// Host setup from the first-run form, as both Spaces apps send it:
    /// validated by the app core, then `Host.setup`.
    func setupRequest(request: AppHostSetupRequest, accountToken: String?) async throws -> HostStatus {
        try await appHostSetup(host: self, request: request, accountToken: accountToken)
    }
}
