// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
import Foundation
import LocalAuthentication
import Observation
import UserNotifications

/// This Mac as a client of the account on the relay: the cua SDK's
/// `Devices` (live) or `FixtureDevices` (fixtures, tests).
public protocol DevicesRunning: AnyObject, Sendable {
    func snapshot() async throws -> DevicesSnapshot
    func enroll() async throws -> DeviceEnrollment
    func checkEnrolled() async -> Bool
    func approve(code: String?, deviceId: String?) async throws
    func rename(id: String, name: String) async throws
    func revoke(id: String) async throws
    /// Vouches for a machine that registered without an enrolled device's
    /// proof (S5), from this enrolled device.
    func confirmMachine(id: String) async throws
}

/// The SDK's `Devices` for the signed-in session (key in the session's vault,
/// shared with the `cua` CLI and daemon).
public final class LiveDevices: DevicesRunning, @unchecked Sendable {
    let inner: CuaSDK.Devices

    public init(inner: CuaSDK.Devices) { self.inner = inner }

    /// This Mac on the default relay (`CUA_RELAY_URL`, else relay.cua.ai),
    /// named as Sharing names it ("Dana's MacBook Pro").
    public static func make(auth: CuaSDK.Auth) -> LiveDevices? {
        (try? auth.devices(relayUrl: nil, name: Foundation.Host.current().localizedName)).map(LiveDevices.init)
    }

    public func snapshot() async throws -> DevicesSnapshot { try await inner.snapshot(auditLimit: 50) }
    public func enroll() async throws -> DeviceEnrollment { try await inner.enroll() }
    public func checkEnrolled() async -> Bool { await inner.checkEnrolled() }
    public func approve(code: String?, deviceId: String?) async throws {
        _ = try await inner.approve(code: code, deviceId: deviceId)
    }
    public func rename(id: String, name: String) async throws { _ = try await inner.rename(id: id, name: name) }
    public func revoke(id: String) async throws { _ = try await inner.revoke(id: id) }
    public func confirmMachine(id: String) async throws { _ = try await inner.confirmMachine(machineId: id) }
}

/// An in-memory account on a relay (fixtures, tests): this Mac enrolled,
/// a new device and an expired one asking, and some access. It never
/// touches the network or the device key.
public final class FixtureDevices: DevicesRunning, @unchecked Sendable {
    public var current: DevicesSnapshot
    public private(set) var calls: [String] = []
    /// What `enroll` answers (a code unless set).
    public var enrollEnrolled = false
    /// What `checkEnrolled` answers.
    public var enrolledNow = false
    public var failApprove: String?

    public init(snapshot: DevicesSnapshot? = nil) {
        current = snapshot ?? FixtureDevices.sample(now: UInt64(Date().timeIntervalSince1970))
    }

    public static func sample(now: UInt64) -> DevicesSnapshot {
        let day: UInt64 = 86_400
        func device(_ id: String, _ name: String, _ state: String, _ platform: String,
                    until: UInt64?, seen: UInt64?, current: Bool = false) -> RelayDevice {
            RelayDevice(id: id, name: name, state: state, platform: platform, createdAt: now - 40 * day,
                        enrolledUntil: until, lastSeen: seen, current: current)
        }
        return DevicesSnapshot(
            localDeviceId: "dev_mac",
            devices: [
                device("dev_mac", "MacBook Pro", "enrolled", "macos", until: now + 24 * day, seen: now - 60,
                       current: true),
                device("dev_studio", "Studio", "enrolled", "linux", until: now + 12 * day, seen: now - 3 * 3600),
                device("dev_work", "Work laptop", "pending", "windows", until: nil, seen: nil),
                device("dev_old", "Old laptop", "expired", "linux", until: now - 2 * day, seen: now - 33 * day),
            ],
            enforceAfter: now - day,
            audit: [
                RelayAuditEvent(ts: now - 5 * 3600, kind: "machine_access", device: "dev_studio", machine: "m1",
                                subject: nil, detail: "owner"),
                RelayAuditEvent(ts: now - 2 * 3600, kind: "shared_access", device: nil, machine: "m1",
                                subject: "bob@example.com", detail: nil),
                RelayAuditEvent(ts: now - 1800, kind: "device_registered", device: "dev_work", machine: nil,
                                subject: nil, detail: nil),
                RelayAuditEvent(ts: now - 600, kind: "machine_access", device: "dev_mac", machine: "m2",
                                subject: nil, detail: "owner"),
            ],
            machineNames: ["m1": "studio-mac", "m2": "build-box"],
            machines: [
                RelayMachine(id: "m1", spaceId: "relay:m1", name: "studio-mac", ownerId: "me",
                             ownerEmail: nil, role: "owner", online: true, sharing: true, version: "0.5.0",
                             url: "", allow: [], clients: [], confirmed: true),
                RelayMachine(id: "m2", spaceId: "relay:m2", name: "build-box", ownerId: "me",
                             ownerEmail: nil, role: "owner", online: false, sharing: true, version: "0.5.0",
                             url: "", allow: [], clients: [], confirmed: false),
            ])
    }

    public func snapshot() async throws -> DevicesSnapshot { current }

    public func enroll() async throws -> DeviceEnrollment {
        calls.append("enroll")
        let me = current.devices.first { $0.current } ?? RelayDevice(
            id: "dev_mac", name: "MacBook Pro", state: "pending", platform: "macos", createdAt: 0,
            enrolledUntil: nil, lastSeen: nil, current: true)
        return DeviceEnrollment(device: me, enrolled: enrollEnrolled, code: enrollEnrolled ? nil : "K7QX-M2RP")
    }

    public func checkEnrolled() async -> Bool {
        calls.append("check")
        return enrolledNow
    }

    public func approve(code: String?, deviceId: String?) async throws {
        calls.append("approve:\(code ?? deviceId ?? "")")
        if let failApprove { throw CuaError.PermissionDenied(message: failApprove) }
        for i in current.devices.indices where current.devices[i].id == deviceId
            || (code != nil && current.devices[i].state == "pending") {
            current.devices[i].state = "enrolled"
            break
        }
    }

    public func rename(id: String, name: String) async throws {
        calls.append("rename:\(id):\(name)")
        for i in current.devices.indices where current.devices[i].id == id { current.devices[i].name = name }
    }

    public func revoke(id: String) async throws {
        calls.append("revoke:\(id)")
        for i in current.devices.indices where current.devices[i].id == id { current.devices[i].state = "revoked" }
    }

    public func confirmMachine(id: String) async throws {
        calls.append("confirm-machine:\(id)")
        for i in current.machines.indices where current.machines[i].id == id { current.machines[i].confirmed = true }
    }
}

/// Presence before approving a device: Touch ID, an Apple Watch or the
/// login password.
public protocol PresenceChecking: Sendable {
    /// Returns when the user confirmed `reason`; throws otherwise.
    func confirm(reason: String) async throws
}

/// LocalAuthentication's owner check, the same policy the cua daemon asks
/// for before the Keyvault widens access.
public struct LivePresence: PresenceChecking {
    public init() {}

    public func confirm(reason: String) async throws {
        let context = LAContext()
        var error: NSError?
        guard context.canEvaluatePolicy(.deviceOwnerAuthentication, error: &error) else {
            throw CuaError.PermissionDenied(message: "Touch ID and the login password are not available")
        }
        do {
            try await context.evaluatePolicy(.deviceOwnerAuthentication, localizedReason: reason)
        } catch let e as LAError where [.userCancel, .systemCancel, .appCancel].contains(e.code) {
            throw CuaError.PermissionDenied(message: "Approval was cancelled")
        } catch {
            throw CuaError.PermissionDenied(message: "Approval was not confirmed")
        }
    }
}

/// A presence answer for tests: yes or no, recording the reasons asked.
public final class FixturePresence: PresenceChecking, @unchecked Sendable {
    public var allow: Bool
    public private(set) var asked: [String] = []
    public init(allow: Bool = true) { self.allow = allow }
    public func confirm(reason: String) async throws {
        asked.append(reason)
        if !allow { throw CuaError.PermissionDenied(message: "Approval was cancelled") }
    }
}

/// Where a Devices sheet shows: the main window or Settings.
public enum DevicesSurface: Sendable {
    case main
    case settings
}

/// Settings → Devices, the enroll sheet, the approval sheet and the main
/// window's banner. Every word and decision is the core's
/// (`appDevicesView`, `appEnroll*`, `appApprove*`); this runs the SDK calls
/// they ask for. Nothing here prompts on its own except a new device's
/// approval (a notification and the sheet): unattended agents keep their
/// device session without the user.
@MainActor
@Observable
public final class DevicesModel {
    let devices: DevicesRunning?
    let presence: PresenceChecking
    /// The relay's answer, once read.
    public private(set) var snapshot: DevicesSnapshot?
    /// The code this Mac shows while it waits for approval.
    public private(set) var pendingCode: String?
    public private(set) var error: String?
    public private(set) var busy = false
    /// The enroll sheet, while it shows, and where.
    public var enroll: AppEnrollState?
    public var enrollSurface: DevicesSurface = .main
    /// The approval sheet, while it shows, and where.
    public var approval: AppApproveSheetState?
    public var approvalSurface: DevicesSurface = .main
    /// A new machine's confirm alert, while it shows (S5).
    public var confirming: AppUnconfirmedMachine?
    /// Signed in to Cua (no account, no devices).
    public var signedIn = false
    /// Runs the interactive sign-in; true once signed in again.
    public var signIn: (() async -> Bool)?
    /// Shows a system notification for a device asking to join.
    public var notify: ((AppApprovalPrompt) -> Void)?
    /// Approvals already announced (a device asks once per launch).
    var announced: Set<String> = []
    /// Re-verifications put off with Not Now.
    var dismissed: Set<String> = []
    /// Now (tests pin it).
    public var clock: () -> Date = { Date() }
    /// How long to wait between enrollment checks while the code shows.
    var pollInterval: Duration = .seconds(3)
    /// At most this many checks (10 minutes, the code's lifetime).
    var pollLimit = 200
    var locale = Locale.current
    var timeZone = TimeZone.current

    public init(devices: DevicesRunning?, presence: PresenceChecking = LivePresence()) {
        self.devices = devices
        self.presence = presence
    }

    var nowSecs: UInt64 { UInt64(max(0, clock().timeIntervalSince1970)) }

    public var input: AppDevicesInput {
        snapshot.map { appDevicesInput(snapshot: $0, pendingCode: pendingCode) }
            ?? AppDevicesInput(devices: [], audit: [], localDeviceId: nil, pendingCode: pendingCode,
                               enforceAfter: nil, machineNames: [:], machines: [])
    }

    /// The Devices page.
    public var view: AppDevicesView { appDevicesView(input: input, now: nowSecs) }

    /// The main window's banner: signed in, read, and something needs the user.
    public var banner: AppDeviceBanner? {
        guard signedIn, snapshot != nil else { return nil }
        return view.banner
    }

    public var enrollView: AppEnrollView? { enroll.map { appEnrollView(state: $0) } }
    public var approvalView: AppApproveSheetView? { approval.map { appApproveView(state: $0, devices: input.devices) } }

    // MARK: - Words the shell formats

    /// "Enrolled until Oct 27, 2026".
    public func thisDeviceText(_ t: AppThisDevice) -> String {
        guard let at = t.at else { return t.title }
        return "\(t.title) \(dateText(at))"
    }

    func dateText(_ secs: UInt64) -> String {
        let f = DateFormatter()
        f.locale = locale
        f.timeZone = timeZone
        f.dateStyle = .medium
        f.timeStyle = .none
        return f.string(from: Date(timeIntervalSince1970: TimeInterval(secs)))
    }

    /// "5 min ago", relative to the model's clock.
    public func relativeText(_ secs: UInt64) -> String {
        let f = RelativeDateTimeFormatter()
        f.locale = locale
        f.unitsStyle = .full
        let date = Date(timeIntervalSince1970: TimeInterval(secs))
        let now = clock()
        return date > now.addingTimeInterval(-60) ? "just now" : f.localizedString(for: date, relativeTo: now)
    }

    /// A row's second line: platform and state, then when it was last seen.
    public func rowDetail(_ r: AppDeviceRow) -> String {
        guard let seen = r.lastSeen else { return r.detail }
        return "\(r.detail) \u{b7} \(view.labels.lastSeen) \(relativeText(seen))"
    }

    // MARK: - Refresh

    /// Reads the relay; announces devices newly asking for approval.
    public func refresh() async {
        guard signedIn, let devices else {
            snapshot = nil
            return
        }
        do {
            snapshot = try await devices.snapshot()
            error = nil
        } catch {
            self.error = LiveSpacesBackend.words(error)
            return
        }
        if view.enrolled { pendingCode = nil }
        for prompt in view.approvals where !announced.contains(prompt.deviceId) {
            announced.insert(prompt.deviceId)
            guard !prompt.expired || !dismissed.contains(prompt.deviceId) else { continue }
            notify?(prompt)
            if approval == nil, enroll == nil { openApproval(prompt, in: .main) }
        }
    }

    // MARK: - Enroll this device

    public func startEnroll(in surface: DevicesSurface) {
        enrollSurface = surface
        enroll = appEnrollInitial()
    }

    public func closeEnroll() { enroll = nil }

    /// Where usage events go (enrolled by a sign-in or an approval, or
    /// failed; the app core's, like the Tauri app).
    public var telemetry: TelemetryRunning?

    func sendEnroll(_ action: AppEnrollAction) {
        guard let s = enroll else { return }
        telemetry?.record(appTelemetryEnroll(state: s, action: action))
        enroll = appEnrollReduce(state: s, action: action)
    }

    public func backEnroll() { sendEnroll(.back) }

    /// "Sign in again" or "Approve from another device".
    public func chooseEnroll(_ method: AppEnrollMethod) async {
        sendEnroll(.choose(method: method))
        if enroll?.phase == .signingIn {
            let ok = await signIn?() ?? false
            guard enroll?.phase == .signingIn else { return }
            if !ok {
                sendEnroll(.failed(error: "Sign-in did not finish."))
                return
            }
            sendEnroll(.signedIn)
        }
        guard enroll?.phase == .registering, let devices else { return }
        do {
            let r = try await devices.enroll()
            sendEnroll(.registered(enrolled: r.enrolled, code: r.code))
            pendingCode = r.enrolled ? nil : r.code
        } catch {
            sendEnroll(.failed(error: LiveSpacesBackend.words(error)))
            return
        }
        await waitForApproval()
        await refresh()
    }

    /// Checks until an enrolled device approves, the sheet closes or the
    /// code runs out.
    func waitForApproval() async {
        guard let devices else { return }
        var checks = 0
        while enroll?.phase == .waiting, checks < pollLimit, !Task.isCancelled {
            checks += 1
            if await devices.checkEnrolled() {
                sendEnroll(.approved)
                pendingCode = nil
                return
            }
            try? await Task.sleep(for: pollInterval)
        }
    }

    // MARK: - Approve another device

    public func openApproval(_ prompt: AppApprovalPrompt, in surface: DevicesSurface) {
        approvalSurface = surface
        approval = appApproveOpen(prompt: prompt)
    }

    /// The row's Approve… (the prompt for that device).
    public func openApproval(deviceId: String, in surface: DevicesSurface) {
        guard let p = view.approvals.first(where: { $0.deviceId == deviceId }) else { return }
        openApproval(p, in: surface)
    }

    public func setCode(_ code: String) {
        guard let s = approval else { return }
        approval = appApproveReduce(state: s, action: .setCode(code: code), devices: input.devices)
    }

    /// Approve: presence first, then the relay. `devices` is the account's
    /// current devices: once a code expires, it is how the sheet finds the
    /// sole waiting device of that name to offer approving by id instead.
    public func approve() async {
        guard let s = approval, let devices else { return }
        let deviceList = input.devices
        let next = appApproveReduce(state: s, action: .submit, devices: deviceList)
        guard next.busy, let request = appApproveView(state: next, devices: deviceList).request else { return }
        approval = next
        do {
            try await presence.confirm(reason: appApproveView(state: next, devices: deviceList).presenceReason)
            try await devices.approve(code: request.code, deviceId: request.deviceId)
        } catch {
            if let cur = approval {
                approval = appApproveReduce(state: cur, action: .failed(error: LiveSpacesBackend.words(error)), devices: deviceList)
            }
            return
        }
        approval = nil
        await refresh()
    }

    /// Deny (revokes a new device, one click) or Not Now (re-verification).
    public func deny() async {
        guard let s = approval else { return }
        let v = appApproveView(state: s, devices: input.devices)
        approval = nil
        guard v.denyRevokes else {
            dismissed.insert(s.deviceId)
            return
        }
        await run { try await $0.revoke(id: s.deviceId) }
    }

    /// The row's Deny (no sheet): revokes a still-pending device in one
    /// click, like the sheet's Deny, or (a device due for re-verification)
    /// only dismisses it, like the sheet's Not Now.
    public func denyApproval(deviceId: String) async {
        guard let p = view.approvals.first(where: { $0.deviceId == deviceId }) else { return }
        guard !p.expired else {
            dismissed.insert(p.deviceId)
            return
        }
        await run { try await $0.revoke(id: p.deviceId) }
    }

    // MARK: - Rows

    public func rename(id: String, to name: String) async {
        guard let clean = appDevicesCleanName(name: name) else { return }
        await run { try await $0.rename(id: id, name: clean) }
    }

    /// After the row's confirmation.
    public func revoke(id: String) async {
        await run { try await $0.revoke(id: id) }
    }

    // MARK: - New machines (S5)

    /// After the row's confirmation.
    public func confirmMachine(id: String) async {
        await run { try await $0.confirmMachine(id: id) }
    }

    func run(_ call: (DevicesRunning) async throws -> Void) async {
        guard let devices, !busy else { return }
        busy = true
        defer { busy = false }
        do {
            try await call(devices)
            error = nil
        } catch {
            self.error = LiveSpacesBackend.words(error)
            return
        }
        await refresh()
    }
}

/// System notifications for devices asking to join. Only a bundled app
/// posts them (a test process has no notification identity).
@MainActor
final class DeviceNotifier: NSObject, UNUserNotificationCenterDelegate {
    static let shared = DeviceNotifier()
    private var authorized: Bool?

    static var available: Bool { Bundle.main.bundleURL.pathExtension == "app" }

    func post(_ prompt: AppApprovalPrompt) {
        guard Self.available else { return }
        let center = UNUserNotificationCenter.current()
        center.delegate = self
        let content = UNMutableNotificationContent()
        content.title = prompt.notifyTitle
        content.body = prompt.notifyBody
        content.sound = .default
        let request = UNNotificationRequest(identifier: "device-\(prompt.deviceId)", content: content, trigger: nil)
        Task {
            if authorized == nil {
                authorized = (try? await center.requestAuthorization(options: [.alert, .sound])) ?? false
            }
            guard authorized == true else { return }
            try? await center.add(request)
        }
    }

    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification)
        async -> UNNotificationPresentationOptions { [.banner, .sound] }

    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter,
                                            didReceive response: UNNotificationResponse) async {
        await MainActor.run { NSApp.activate() }
    }
}
