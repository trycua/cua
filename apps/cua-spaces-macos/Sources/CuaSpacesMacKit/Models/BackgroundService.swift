// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation

/// What closing the window and quitting do to the background service
/// (the host service login item, `HelperService`, which runs cua-spacesd;
/// Cua Driver's login item, `DriverService`; and the `cua daemon`).
///
/// * Closing the main window leaves all of it running, so agents keep
///   creating and managing Spaces. Settings, General, "When I close the
///   window" can make the close button quit instead; the first close (and
///   each one until the person turns it off) shows a notice.
/// * Quit (the menu bar item, or Cmd-Q) stops the app and all of it.
/// * Quit for an update does not stop the service: it restarts itself onto
///   the new build (`HelperService.watch`), so agents lose nothing but a
///   few seconds.
public enum BackgroundPolicy {
    /// What a closed main window leads to.
    public enum CloseAction: Equatable, Sendable {
        /// Nothing more (the app is already quitting, or the notice is off).
        case nothing
        /// The notice that Cua Spaces is still running.
        case showNotice
        /// The setting says quit: quit as Quit does.
        case quitApp
    }

    /// `keepsRunning` is Settings' "When I close the window" (keep, the
    /// default); `noticeOff` the person's "Don't show this again";
    /// `terminating`: the app is already quitting (its windows close).
    public static func windowClosed(keepsRunning: Bool, noticeOff: Bool, terminating: Bool) -> CloseAction {
        if terminating { return .nothing }
        if !keepsRunning { return .quitApp }
        return noticeOff ? .nothing : .showNotice
    }

    /// What a quit does to the background service.
    public enum QuitPlan: Equatable, Sendable {
        /// Stop the service, the driver and the cua daemon, then exit.
        case stopEverything
        /// An update restart, or a build that runs no service (fixtures):
        /// leave it as it is.
        case leaveRunning
    }

    public static func quitPlan(updateRestart: Bool, managesService: Bool) -> QuitPlan {
        (updateRestart || !managesService) ? .leaveRunning : .stopEverything
    }

    /// The notice's words (Settings, General has the same setting).
    public static let noticeText =
        "Cua Spaces is still running in the background so your agents can create and manage their Spaces."
    public static let noticeTitle = "Cua Spaces is still running"
}

/// The state of one quit, and the stop itself.
public final class BackgroundService: @unchecked Sendable {
    public static let shared = BackgroundService()

    private let lock = NSLock()
    private var updateRestart = false
    private var quitting = false
    /// The app's own daemon supervisor; `nil` in a fixtures run, which never
    /// touches the host (nothing is stopped).
    private var supervisor: DaemonSupervisor?

    public init() {}

    public func manage(_ supervisor: DaemonSupervisor?) { lock.withLock { self.supervisor = supervisor } }

    /// The app is about to restart onto an update: the service keeps
    /// running through it.
    public func markUpdateRestart() { lock.withLock { updateRestart = true } }

    /// The app is quitting (its windows closing must not show a notice).
    public var isQuitting: Bool { lock.withLock { quitting } }

    /// Starts a quit: what to do with the service.
    public func beginQuit() -> BackgroundPolicy.QuitPlan {
        lock.withLock {
            quitting = true
            let plan = BackgroundPolicy.quitPlan(updateRestart: updateRestart, managesService: supervisor != nil)
            if plan == .stopEverything { supervisor?.stopSupervising() }
            return plan
        }
    }

    /// A quit that did not happen (a sheet cancelled it): the app goes on.
    public func cancelQuit() {
        lock.withLock {
            quitting = false
            supervisor?.resumeSupervising()
        }
    }

    /// `launchctl kill SIGTERM` on each login item (each exits 0 on SIGTERM,
    /// so launchd leaves it stopped), then `cua daemon stop`; waits for the
    /// host service to be gone, at most `wait` seconds. Returns what could
    /// not be stopped, in plain words (empty: all stopped). Blocks: call off
    /// the main thread.
    public func stopEverything(runner: CommandRunning = ProcessRunner(),
                               labels: [String] = [HelperService.label, DriverService.label],
                               uid: uid_t = getuid(),
                               wait: TimeInterval = 8,
                               isRunning: ((String) -> Bool)? = nil) -> [String] {
        let isRunning = isRunning ?? { HelperService.runningPid(target: "gui/\(getuid())/\($0)") != nil }
        var problems: [String] = []
        for label in labels {
            _ = runner.run("/bin/launchctl", ["kill", "SIGTERM", "gui/\(uid)/\(label)"], timeout: 5)
        }
        if let supervisor = lock.withLock({ self.supervisor }) {
            if let why = supervisor.stopForQuit(runner: runner) { problems.append(why) }
        }
        let deadline = Date().addingTimeInterval(wait)
        while Date() < deadline, labels.contains(where: isRunning) { Thread.sleep(forTimeInterval: 0.2) }
        for label in labels where isRunning(label) {
            problems.append("\(label) is still running")
        }
        return problems
    }
}
