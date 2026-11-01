// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation

/// Settings, General's "Repo and dependency cache": `repoCache.get` (on or
/// off, what it holds and what the last week saved, in the host's words),
/// `repoCache.set` (the switch and the size in GB, `null` for automatic) and
/// `repoCache.clear`. The cache itself belongs to the Cua daemon.
extension WebUIBridge {
    func repoCache(_ method: String, _ args: [String: Any]) async throws -> Any {
        let state: RepoCacheState?
        do {
            switch method {
            case "repoCache.get":
                state = await model.backend.repoCacheStatus()
            case "repoCache.clear":
                state = try await model.backend.clearRepoCache()
            default:
                var enabled: Bool?
                if let v = args["enabled"], !(v is NSNull) {
                    guard let b = v as? Bool, !(v is NSNumber) || CFGetTypeID(v as CFTypeRef) == CFBooleanGetTypeID() else {
                        throw Failure.badArgs("enabled: true or false")
                    }
                    enabled = b
                }
                var size: String?
                if let v = args["sizeGb"] {
                    if v is NSNull {
                        size = "auto"
                    } else {
                        guard let n = (v as? NSNumber)?.doubleValue, n.isFinite, n >= 0, n <= 100_000 else {
                            throw Failure.badArgs("sizeGb: a number of GB, 0 or more, or null for automatic")
                        }
                        size = n == n.rounded() ? String(UInt64(n)) : String(n)
                    }
                }
                state = try await model.backend.setRepoCache(enabled: enabled, size: size)
            }
        } catch let failure as Failure {
            throw failure
        } catch {
            throw Failure(code: "failed", message: LiveSpacesBackend.words(error))
        }
        guard let s = state else { throw Failure.unsupported("The repo cache is not available in this build") }
        return [
            "enabled": s.enabled, "repos": Int(s.repos), "bytes": s.bytes, "capBytes": s.capBytes,
            "sizeSetting": s.sizeSetting, "summary": s.summary, "week": s.week,
            "hits": Int(s.hits), "misses": Int(s.misses), "secondsSaved": s.secondsSaved,
        ] as [String: Any]
    }
}
