// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import Foundation
import Sparkle

/// What the page shows about an update, the JSON behind the bridge's
/// `update.get`:
/// `{status, currentVersion, version, percent, notesUrl, message}`.
public struct UpdateState: Equatable, Sendable {
    public enum Status: String, Sendable {
        case idle, checking, available, downloading, downloaded, error
    }

    public var status: Status
    /// This app's full version.
    public var currentVersion: String
    /// The update's version (the appcast item's display version).
    public var version: String?
    /// 0 to 100 while downloading, else nil.
    public var percent: Int?
    public var notesUrl: String?
    public var message: String?

    public init(status: Status = .idle, currentVersion: String, version: String? = nil, percent: Int? = nil,
                notesUrl: String? = nil, message: String? = nil) {
        self.status = status
        self.currentVersion = currentVersion
        self.version = version
        self.percent = percent
        self.notesUrl = notesUrl
        self.message = message
    }

    public static func idle(currentVersion: String) -> UpdateState { UpdateState(currentVersion: currentVersion) }

    /// The release's page: the repository's tag for a Cua Spaces version;
    /// for a nightly, the rolling nightly release (its notes list the changes).
    public static func notesURL(version: String) -> String {
        if version.split(separator: "+").first?.contains("-nightly") == true {
            return "https://github.com/trycua/cua/releases/tag/cua-spaces-nightly"
        }
        return "https://github.com/trycua/cua/releases/tag/cua-spaces-v\(version)"
    }

    /// The bridge's record; absent values are explicit nulls.
    public var json: [String: Any] {
        [
            "status": status.rawValue,
            "currentVersion": currentVersion,
            "version": version.map { $0 as Any } ?? NSNull(),
            "percent": percent.map { $0 as Any } ?? NSNull(),
            "notesUrl": notesUrl.map { $0 as Any } ?? NSNull(),
            "message": message.map { $0 as Any } ?? NSNull(),
        ]
    }
}

/// The update state machine over plain values (no Sparkle types), so each
/// step is testable. `UpdateUserDriver` feeds it Sparkle's callbacks.
public struct UpdateFlow: Equatable, Sendable {
    public private(set) var state: UpdateState
    /// A check the user started (its errors are shown).
    public private(set) var userCheck = false
    private var received: UInt64 = 0
    private var expected: UInt64 = 0

    public init(currentVersion: String) { state = .idle(currentVersion: currentVersion) }

    /// The download takes the first 90 percent of the bar, extraction the rest.
    private static let downloadShare = 90.0

    /// A user-visible check begins.
    public mutating func userCheckStarted() {
        userCheck = true
        state = UpdateState(status: .checking, currentVersion: state.currentVersion)
    }

    /// An update was found; it downloads at once.
    public mutating func found(version: String) {
        received = 0
        expected = 0
        state = UpdateState(status: .available, currentVersion: state.currentVersion, version: version,
                            notesUrl: UpdateState.notesURL(version: version))
    }

    public mutating func downloadStarted() {
        received = 0
        expected = 0
        setPercent(0)
    }

    public mutating func expectedLength(_ length: UInt64) {
        expected = length
        setPercent(downloadPercent)
    }

    public mutating func receivedData(_ length: UInt64) {
        received += length
        setPercent(downloadPercent)
    }

    public mutating func extractionStarted() { setPercent(Int(Self.downloadShare)) }

    /// `progress` is 0 to 1.
    public mutating func extractionProgress(_ progress: Double) {
        let p = min(max(progress.isFinite ? progress : 0, 0), 1)
        setPercent(Int((Self.downloadShare + (100 - Self.downloadShare) * p).rounded()))
    }

    /// Downloaded, extracted, and waiting for the user's restart.
    public mutating func downloaded() {
        state.status = .downloaded
        state.percent = nil
        state.message = nil
    }

    public mutating func notFound() {
        userCheck = false
        state = .idle(currentVersion: state.currentVersion)
    }

    public mutating func failed(_ message: String) {
        userCheck = false
        state = UpdateState(status: .error, currentVersion: state.currentVersion, message: message)
    }

    /// Sparkle ended its session (after an install, an error, or no update).
    public mutating func dismissed() {
        userCheck = false
        if state.status == .error { return }
        state = .idle(currentVersion: state.currentVersion)
    }

    private var downloadPercent: Int {
        guard expected > 0 else { return 0 }
        return Int((Double(min(received, expected)) / Double(expected) * Self.downloadShare).rounded())
    }

    private mutating func setPercent(_ percent: Int) {
        guard state.version != nil else { return }
        state.status = .downloading
        // Never backwards.
        state.percent = max(state.percent ?? 0, min(max(percent, 0), 100))
    }
}

/// Sparkle's user driver without windows: a background update downloads on
/// its own and waits for the page's "Restart to update". Sparkle's
/// ready-to-install reply is held until `installNow`; nothing restarts the
/// app before that.
@MainActor
public final class UpdateUserDriver: NSObject, SPUUserDriver {
    public private(set) var flow: UpdateFlow
    /// Told after every state change.
    public var onChange: (() -> Void)?
    /// `downloaded` when the update is ready to install.
    public var onEvent: ((String) -> Void)?
    /// Shows an error to the person who started the check.
    var presentError: (String) -> Void = UpdateUserDriver.alertError

    /// Sparkle's held "ready to install" reply.
    private var heldReply: ((SPUUserUpdateChoice) -> Void)?
    /// The silent path's immediate install (`willInstallUpdateOnQuit`).
    private var immediateInstall: (() -> Void)?
    private var told = false

    public var state: UpdateState { flow.state }

    public init(currentVersion: String) {
        flow = UpdateFlow(currentVersion: currentVersion)
        super.init()
    }

    private func changed(_ edit: (inout UpdateFlow) -> Void) {
        let before = flow
        edit(&flow)
        if flow != before { onChange?() }
    }

    /// The user pressed Check Now.
    public func beginUserCheck() { changed { $0.userCheckStarted() } }

    /// Both paths end here: the update waits for the user.
    private func becameReady() {
        changed { $0.downloaded() }
        if !told {
            told = true
            onEvent?("downloaded")
        }
    }

    /// The silent path (`automaticallyDownloadsUpdates`): Sparkle installs on
    /// quit, or at once through `install`.
    public func silentlyDownloaded(version: String, install: @escaping () -> Void) {
        immediateInstall = install
        if flow.state.version == nil { changed { $0.found(version: version) } }
        becameReady()
    }

    /// Restart to update: the held reply, or the silent path's install.
    /// False when nothing is downloaded.
    @discardableResult
    public func installNow() -> Bool {
        guard flow.state.status == .downloaded else { return false }
        if let reply = heldReply {
            heldReply = nil
            reply(.install)
            return true
        }
        if let install = immediateInstall {
            install()
            return true
        }
        return false
    }

    /// The update cycle ended; a check nobody saw end is idle.
    public func cycleFinished() {
        if flow.state.status == .checking { changed { $0.dismissed() } }
    }

    // MARK: - Update found

    /// What `showUpdateFound` does, over plain values.
    func handleFound(version: String, informationOnly: Bool, reply: @escaping (SPUUserUpdateChoice) -> Void) {
        // An information-only item has nothing to install.
        guard !informationOnly else {
            changed { $0.notFound() }
            reply(.dismiss)
            return
        }
        told = false
        changed { $0.found(version: version) }
        reply(.install)
    }

    public func showUpdateFound(with appcastItem: SUAppcastItem, state: SPUUserUpdateState,
                                reply: @escaping (SPUUserUpdateChoice) -> Void) {
        handleFound(version: appcastItem.displayVersionString, informationOnly: appcastItem.isInformationOnlyUpdate,
                    reply: reply)
    }

    // MARK: - SPUUserDriver

    public func show(_ request: SPUUpdatePermissionRequest, reply: @escaping (SUUpdatePermissionResponse) -> Void) {
        // Checking and downloading are on by default (SUEnableAutomaticChecks,
        // SUAutomaticallyUpdate); a user's own choice is kept. Nothing
        // restarts the app without a click on "Restart to update".
        reply(SUUpdatePermissionResponse(automaticUpdateChecks: true, automaticUpdateDownloading: true as NSNumber,
                                         sendSystemProfile: false))
    }

    public func showUserInitiatedUpdateCheck(cancellation: @escaping () -> Void) { beginUserCheck() }

    public func showUpdateReleaseNotes(with downloadData: SPUDownloadData) {}
    public func showUpdateReleaseNotesFailedToDownloadWithError(_ error: any Error) {}

    public func showUpdateNotFoundWithError(_ error: any Error, acknowledgement: @escaping () -> Void) {
        changed { $0.notFound() }
        acknowledgement()
    }

    public func showUpdaterError(_ error: any Error, acknowledgement: @escaping () -> Void) {
        handleError(error.localizedDescription)
        acknowledgement()
    }

    /// A background error is logged only; the check's user sees an alert.
    func handleError(_ message: String) {
        let shown = flow.userCheck
        NSLog("Cua Spaces: the updater failed: %@", message)
        changed { $0.failed(message) }
        if shown { presentError(message) }
    }

    public func showDownloadInitiated(cancellation: @escaping () -> Void) { changed { $0.downloadStarted() } }
    public func showDownloadDidReceiveExpectedContentLength(_ expectedContentLength: UInt64) {
        changed { $0.expectedLength(expectedContentLength) }
    }
    public func showDownloadDidReceiveData(ofLength length: UInt64) { changed { $0.receivedData(length) } }
    public func showDownloadDidStartExtractingUpdate() { changed { $0.extractionStarted() } }
    public func showExtractionReceivedProgress(_ progress: Double) { changed { $0.extractionProgress(progress) } }

    public func showReady(toInstallAndRelaunch reply: @escaping (SPUUserUpdateChoice) -> Void) {
        // Held until installNow: replying here would restart the app.
        heldReply = reply
        becameReady()
    }

    public func showInstallingUpdate(withApplicationTerminated applicationTerminated: Bool,
                                     retryTerminatingApplication: @escaping () -> Void) {
        // The page already said "Restarting"; the app quits on its own.
        if !applicationTerminated { return }
    }

    public func showUpdateInstalledAndRelaunched(_ relaunched: Bool, acknowledgement: @escaping () -> Void) {
        acknowledgement()
    }

    public func showUpdateInFocus() {}

    public func dismissUpdateInstallation() {
        heldReply = nil
        immediateInstall = nil
        told = false
        changed { $0.dismissed() }
    }

    private static func alertError(_ message: String) {
        let alert = NSAlert()
        alert.messageText = "Update Error!"
        alert.informativeText = message
        alert.addButton(withTitle: "OK")
        NSApp.activate()
        alert.runModal()
    }
}
