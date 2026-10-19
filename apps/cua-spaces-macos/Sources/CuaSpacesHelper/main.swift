// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// "Cua Spaces Helper": the main executable of
// Cua Spaces.app/Contents/Library/LoginItems/Cua Spaces Helper.app, the
// host service macOS runs as the app's login item (SMAppService.agent, plist
// Contents/Library/LaunchAgents/com.trycua.spaces.helper.plist).
//
// It runs only its sibling `cua-spacesd` (the pinned host service build,
// sealed in this bundle) as a child. launchd starts this launcher, so macOS
// holds it responsible for the child: Screen Recording, Accessibility and
// Local Network are granted to "Cua Spaces Helper" (bundle id
// com.trycua.spaces.helper, the Cua team), once, and stay granted across
// updates.
//
//   Cua Spaces Helper                 the service: reads ~/.cua/helper/service.json
//                                     (written by cua-host: args, env, log,
//                                     working directory), runs cua-spacesd
//                                     with them, restarts on failure through
//                                     launchd (KeepAlive SuccessfulExit=false)
//   Cua Spaces Helper <args...>       runs `cua-spacesd <args...>` with this
//                                     terminal's stdio (`check-permissions`:
//                                     the helper's own TCC status)
//
// No spec (the host is not set up, or was removed): exits 0, and launchd
// leaves it stopped until cua-host writes one and kickstarts it.

import ApplicationServices
import CoreGraphics
import Darwin
import Foundation

/// The spec file cua-host writes (`cua_host::service::helper_spec_path`).
func specPath() -> String {
    var home = ProcessInfo.processInfo.environment["HOME"] ?? ""
    if home.isEmpty, let pw = getpwuid(getuid()), let dir = pw.pointee.pw_dir {
        home = String(cString: dir)
    }
    return home + "/.cua/helper/service.json"
}

/// `cua-spacesd` next to this executable (never a path from the spec: only
/// the code sealed in this bundle runs with the helper's permissions).
func daemonPath() -> String {
    let me = Bundle.main.executablePath ?? CommandLine.arguments[0]
    return URL(fileURLWithPath: me).resolvingSymlinksInPath()
        .deletingLastPathComponent().appendingPathComponent("cua-spacesd").path
}

func log(_ message: String) {
    FileHandle.standardError.write(Data("cua-spaces-helper: \(message)\n".utf8))
}

/// What the service runs (`cua_host::service::ServiceSpec`, serde names).
struct Spec {
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
func spawn(_ args: [String], env extra: [(String, String)], logPath: String?, workingDir: String?) -> pid_t? {
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

func wait(_ pid: pid_t) -> Int32 {
    var status: Int32 = 0
    while waitpid(pid, &status, 0) < 0 {
        if errno != EINTR { return 1 }
    }
    // WIFEXITED / WEXITSTATUS (the macros are not imported into Swift).
    if status & 0x7f == 0 { return (status >> 8) & 0xff }
    return 128 + (status & 0x7f)
}

let passthrough = Array(CommandLine.arguments.dropFirst()).filter { !$0.hasPrefix("-psn_") }

if !passthrough.isEmpty {
    // A command: `cua-spacesd <args>` with this process's stdio, so its
    // permission checks answer for this helper.
    guard let pid = spawn(passthrough, env: [], logPath: nil, workingDir: nil) else { exit(127) }
    exit(wait(pid))
}

guard let spec = Spec.load(specPath()) else {
    log("no host service set up (\(specPath())); nothing to run")
    exit(0)
}
guard !spec.args.isEmpty else {
    log("\(specPath()) has no arguments; nothing to run")
    exit(0)
}

// Stop (launchctl kill SIGTERM, logout, shutdown) reaches the child, and
// this helper then exits 0 so launchd leaves it stopped. Any other end of
// the child is a failure launchd restarts (as the old LaunchAgent's
// KeepAlive did).
nonisolated(unsafe) var child: pid_t = 0
nonisolated(unsafe) var stopping: sig_atomic_t = 0
for sig in [SIGTERM, SIGINT, SIGHUP] {
    signal(sig) { s in
        stopping = 1
        if child > 0 { kill(child, s) }
    }
}
// Sharing the desktop needs Screen Recording and Accessibility for this
// helper. Ask now, as "Cua Spaces Helper" (macOS shows each prompt once per
// app and adds the helper to both lists in System Settings); a background
// capture alone is refused without a prompt.
if !spec.args.contains("--no-desktop") {
    if !CGPreflightScreenCaptureAccess() { _ = CGRequestScreenCaptureAccess() }
    let prompt = "AXTrustedCheckOptionPrompt" // kAXTrustedCheckOptionPrompt
    _ = AXIsProcessTrustedWithOptions([prompt: true] as CFDictionary)
}
guard let pid = spawn(spec.args, env: spec.env, logPath: spec.logPath, workingDir: spec.workingDir) else {
    exit(1)
}
child = pid
// The app was replaced by one without this helper (a downgrade) or the
// host was removed: the files are gone while this process still runs from
// them. Stop, so the service that replaced it can take the port; launchd
// does not start a helper whose program is gone.
let specFile = specPath()
let daemonFile = daemonPath()
Thread.detachNewThread {
    var misses = 0
    while true {
        sleep(5)
        let gone = !FileManager.default.fileExists(atPath: daemonFile)
            || !FileManager.default.fileExists(atPath: specFile)
        misses = gone ? misses + 1 : 0
        if misses >= 2 {
            log("the app's helper or its spec is gone; stopping")
            stopping = 1
            kill(pid, SIGTERM)
            return
        }
    }
}
let code = wait(pid)
exit(stopping != 0 ? 0 : (code == 0 ? 1 : code))
