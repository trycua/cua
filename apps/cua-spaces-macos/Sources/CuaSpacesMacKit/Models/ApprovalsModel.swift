// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
import Foundation
import Observation

/// Settings → Permissions. The words, the rows and the decision to change
/// one are the core's (`ApprovalsPane`: it reads and writes
/// `<cua home>/approvals.json`, the file the MCP server enforces, and asks for
/// Touch ID or the login password before it writes). This only shows the
/// answer: a toggle flips while the prompt is up and goes back if declined.
@MainActor
@Observable
public final class ApprovalsModel {
    let pane: ApprovalsPane
    public private(set) var view: ApprovalsView
    /// Rows whose change waits for the fingerprint, with the value asked for.
    public private(set) var pending: [String: Bool] = [:]
    /// Why the last change did not happen.
    public private(set) var error: String?

    public init(pane: ApprovalsPane) {
        self.pane = pane
        view = pane.view()
    }

    /// The live pane for this Mac's Cua home.
    public static func live() -> ApprovalsModel { ApprovalsModel(pane: ApprovalsPane(cuaHome: nil)) }

    /// A pane over a throwaway home whose approval answers `accept`
    /// (fixtures, tests).
    public static func fixture(home: String, accept: Bool = true) -> ApprovalsModel {
        ApprovalsModel(pane: ApprovalsPane.fixture(home: home, accept: accept))
    }

    /// What a row's switch shows: the asked-for value while the prompt is up.
    public func requires(_ row: ApprovalRow) -> Bool { pending[row.id] ?? row.require }

    /// Rereads the file (an edit elsewhere shows).
    public func reload() { view = pane.view() }

    /// Asks the core to change `row`; the toggle reverts when declined.
    public func set(_ row: ApprovalRow, to require: Bool) async {
        guard pending[row.id] == nil, require != row.require else { return }
        error = nil
        pending[row.id] = require
        do {
            view = try await pane.set(id: row.id, require: require)
        } catch {
            self.error = LiveSpacesBackend.words(error)
        }
        pending[row.id] = nil
    }
}
