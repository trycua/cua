// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpaces
import Foundation

/// Creating a Space, through the generated cua SDK.
///
/// The Swift overlay's `SpacesConnection.createSpace(options:)` takes where,
/// what kind and which engine, but not CPUs or memory. The generated `Spaces`
/// object's `create(options:)` does, and the overlay hands it out as
/// `SpacesConnection.native`, so the wizard calls it directly.
extension SDKSpacesClient: SpaceCreating {
    func createSpace(_ request: SpaceCreateRequest) async throws -> String {
        guard let spaces = sdkConnection.native else { throw SpaceCreationError.noSpacesRuntime }
        let created = try await spaces.create(options: CuaSDK.SpaceCreateOptions(
            image: request.image, on: request.on.rawValue, kind: request.kind.rawValue,
            runtime: request.runtime.rawValue, name: request.name,
            cpus: request.cpus.map(UInt32.init), memoryMb: request.memoryMB.map(UInt64.init),
            wait: true, spacesd: request.spacesd))
        guard let id = created.space?.id ?? created.pendingId, !id.isEmpty else {
            throw SpaceCreationError.noSpaceID
        }
        return id
    }
}
