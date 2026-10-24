// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The host service (cua-spacesd) run by this app's own executable, so macOS
// asks for and keeps every permission under one name, "Cua Spaces" (bundle
// id com.trycua.spaces.macos, the Cua team):
//
//   CuaSpacesMac --service-helper run   the login item (SMAppService.agent,
//                                       Contents/Library/LaunchAgents/
//                                       <CuaHelperLabel>.plist, by default
//                                       com.trycua.spaces.helper.plist):
//                                       reads <CuaHome>/helper/service.json
//                                       (by default ~/.cua/helper/
//                                       service.json; written by cua-host:
//                                       args, env, log, working directory)
//                                       and runs Contents/MacOS/cua-spacesd
//                                       with it
//   CuaSpacesMac check-permissions      `cua-spacesd check-permissions` with
//                                       this terminal's stdio: the app's own
//                                       permission state (cua-host's
//                                       preflight runs it disclaimed)
//
// launchd starts this executable, so macOS holds it (the app's identity)
// responsible for its cua-spacesd child: Screen Recording, Accessibility
// and Local Network checks are all made against "Cua Spaces". A separate
// helper bundle would not do: macOS lists a login item's Accessibility
// under the helper's own name but its Screen Recording under the app's,
// which put two names in System Settings.
//
// No spec (the host is not set up, or was removed): exits 0, and launchd
// leaves it stopped until cua-host writes one and kickstarts it. Never
// touches AppKit, so it never shows in the Dock.

import ApplicationServices
import CoreGraphics
import CuaSpacesMacKit
import Darwin
import Foundation

/// The spec file cua-host writes (`cua_host::service::helper_spec_path`):
/// below the `CuaHome` this bundle was stamped with (Cua Spaces Local:
/// `~/.cua-local`), else `~/.cua`. `nil` when a bundle of another channel
/// has no usable home: it must not read the released app's spec.
private func specPath() -> String? {
    var home = ProcessInfo.processInfo.environment["HOME"] ?? ""
    if home.isEmpty, let pw = getpwuid(getuid()), let dir = pw.pointee.pw_dir {
        home = String(cString: dir)
    }
    return HelperService.specPath(info: Bundle.main.infoDictionary, home: home)
}

/// `cua-spacesd` next to this executable (never a path from the spec: only
/// the code sealed in this app runs with the app's permissions).
private func daemonPath() -> String {
    let me = Bundle.main.executablePath ?? CommandLine.arguments[0]
    return URL(fileURLWithPath: me).resolvingSymlinksInPath()
        .deletingLastPathComponent().appendingPathComponent("cua-spacesd").path
}

private func log(_ message: String) {
    FileHandle.standardError.write(Data("cua-spaces service: \(message)\n".utf8))
}

/// What the service runs (`cua_host::service::ServiceSpec`, serde names).
private struct Spec {
    var args: [String]
    var env: [(String, String)]
    var logPath: String?
    var workingDir: String?

    static func load(_ path: String) -> Spec? {
        guard let data = FileManager.default.contents(atPath: path) else { return nil }
        guard let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            log("\(path) is not valid JSON")
            return nil
        }
        let args = (json["args"] as? [Any])?.compactMap { $0 as? String } ?? []
        var env: [(String, String)] = []
        for pair in json["env"] as? [Any] ?? [] {
            if let kv = pair as? [Any], kv.count == 2, let k = kv[0] as? String, let v = kv[1] as? String {
                env.append((k, v))
            }
        }
        return Spec(args: args, env: env,
                    logPath: (json["log_path"] as? String).flatMap { $0.isEmpty ? nil : $0 },
                    workingDir: (json["working_dir"] as? String).flatMap { $0.isEmpty ? nil : $0 })
    }
}

/// Spawns `cua-spacesd args` (stdio redirected to `logPath` when set) and
/// returns its pid.
private func spawn(_ args: [String], env extra: [(String, String)], logPath: String?, workingDir: String?) -> pid_t? {
    let program = daemonPath()
    guard FileManager.default.isExecutableFile(atPath: program) else {
        log("missing \(program)")
        return nil
    }
    var actions: posix_spawn_file_actions_t?
    posix_spawn_file_actions_init(&actions)
    defer { posix_spawn_file_actions_destroy(&actions) }
    if let logPath {
        let dir = (logPath as NSString).deletingLastPathComponent
        try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
        posix_spawn_file_actions_addopen(&actions, 0, "/dev/null", O_RDONLY, 0)
        posix_spawn_file_actions_addopen(&actions, 1, logPath, O_WRONLY | O_CREAT | O_APPEND, 0o600)
        posix_spawn_file_actions_adddup2(&actions, 1, 2)
    }
    if let workingDir {
        posix_spawn_file_actions_addchdir(&actions, workingDir)
    }
    var environment = ProcessInfo.processInfo.environment
    for (k, v) in extra { environment[k] = v }
    let argv: [UnsafeMutablePointer<CChar>?] = ([program] + args).map { strdup($0) } + [nil]
    let envp: [UnsafeMutablePointer<CChar>?] = environment.map { strdup("\($0.key)=\($0.value)") } + [nil]
    defer {
        argv.forEach { free($0) }
        envp.forEach { free($0) }
    }
    var pid: pid_t = 0
    let rc = posix_spawn(&pid, program, &actions, nil, argv, envp)
    guard rc == 0 else {
        log("could not start \(program): \(String(cString: strerror(rc)))")
        return nil
    }
    return pid
}

private func wait(_ pid: pid_t) -> Int32 {
    var status: Int32 = 0
    while waitpid(pid, &status, 0) < 0 {
        if errno != EINTR { return 1 }
    }
    // WIFEXITED / WEXITSTATUS (the macros are not imported into Swift).
    if status & 0x7f == 0 { return (status >> 8) & 0xff }
    return 128 + (status & 0x7f)
}


// Stop (launchctl kill SIGTERM, logout, shutdown) reaches the child, and
// the service then exits 0 so launchd leaves it stopped. Any other end of
// the child is a failure launchd restarts (KeepAlive SuccessfulExit=false).
private var serviceChild: pid_t = 0
private var serviceStopping: sig_atomic_t = 0

enum ServiceLauncher {
    /// `CuaSpacesMac check-permissions` (and any other cua-spacesd command):
    /// runs it with this process's stdio and exits with its status.
    static func passthrough(_ args: [String]) -> Never {
        guard let pid = spawn(args, env: [], logPath: nil, workingDir: nil) else { exit(127) }
        exit(wait(pid))
    }

    /// `CuaSpacesMac --service-helper run`: the host service.
    static func serve() -> Never {
        guard let specFile = specPath() else {
            log("this app's CuaHome is not usable; nothing to run")
            exit(0)
        }
        guard let spec = Spec.load(specFile) else {
            log("no host service set up (\(specFile)); nothing to run")
            exit(0)
        }
        guard !spec.args.isEmpty else {
            log("\(specFile) has no arguments; nothing to run")
            exit(0)
        }
        for sig in [SIGTERM, SIGINT, SIGHUP] {
            signal(sig) { s in
                serviceStopping = 1
                if serviceChild > 0 { kill(serviceChild, s) }
            }
        }
        // Sharing the desktop needs Screen Recording and Accessibility. Ask
        // now, as "Cua Spaces" (macOS shows each prompt once and lists the
        // app in both panes); a background capture alone is refused without
        // a prompt.
        if !spec.args.contains("--no-desktop") {
            if !CGPreflightScreenCaptureAccess() { _ = CGRequestScreenCaptureAccess() }
            let prompt = "AXTrustedCheckOptionPrompt" // kAXTrustedCheckOptionPrompt
            _ = AXIsProcessTrustedWithOptions([prompt: true] as CFDictionary)
        }
        guard let pid = spawn(spec.args, env: spec.env, logPath: spec.logPath, workingDir: spec.workingDir) else {
            exit(1)
        }
        serviceChild = pid
        // The app was replaced by one without the service (a downgrade) or
        // the host was removed: the files are gone while this process still
        // runs from them. Stop, so the service that replaced it can take the
        // port; launchd does not start a program that is gone.
        let daemonFile = daemonPath()
        Thread.detachNewThread {
            var misses = 0
            while true {
                sleep(5)
                let gone = !FileManager.default.fileExists(atPath: daemonFile)
                    || !FileManager.default.fileExists(atPath: specFile)
                misses = gone ? misses + 1 : 0
                if misses >= 2 {
                    log("the app's cua-spacesd or its spec is gone; stopping")
                    serviceStopping = 1
                    kill(pid, SIGTERM)
                    return
                }
            }
        }
        let code = wait(pid)
        exit(serviceStopping != 0 ? 0 : (code == 0 ? 1 : code))
    }
}
