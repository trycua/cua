import CryptoKit
import Darwin
import Foundation

/// Prepares a freshly-installed macOS VM without driving Setup Assistant over VNC.
///
/// The patcher edits the guest Data volume offline:
/// - creates/updates the local `lume` admin user
/// - marks Setup Assistant complete
/// - enables autologin and SSH
/// - disables screensaver lock and sleep timers
///
/// Some macOS system plists are edited in place because launchd/loginwindow may ignore
/// atomically-replaced files on the next boot even when their contents look correct.
@MainActor
final class MacOSOfflineSetupPatcher {
    private let fileManager: FileManager

    init(fileManager: FileManager = .default) {
        self.fileManager = fileManager
    }

    func patch(vm: VM) throws {
        let diskPath = vm.vmDirContext.diskPath.path
        try ensureStopped(vm: vm)

        Logger.info("Starting offline macOS unattended setup", metadata: [
            "name": vm.name,
            "diskPath": diskPath
        ])

        let mount = try attachDataVolume(diskPath: diskPath)
        defer {
            cleanup(mount: mount)
        }

        try patchMountedDataVolume(at: mount.mountPoint)

        Logger.info("Offline macOS unattended setup completed", metadata: [
            "name": vm.name,
            "mountPoint": mount.mountPoint.path
        ])
    }

    private func ensureStopped(vm: VM) throws {
        let handle = try FileHandle(forReadingFrom: vm.vmDirContext.dir.configPath.url)
        defer { try? handle.close() }

        guard flock(handle.fileDescriptor, LOCK_EX | LOCK_NB) == 0 else {
            throw VMError.alreadyRunning(vm.name)
        }
        flock(handle.fileDescriptor, LOCK_UN)
    }

    // MARK: - Disk Attach/Mount

    private struct MountedDataVolume {
        let session: DiskImageSession
        let mountPoint: URL
    }

    private func attachDataVolume(diskPath: String) throws -> MountedDataVolume {
        let session = DiskImageSession(diskPath: diskPath)
        let wholeDisk = try session.attach(readWrite: true)

        do {
            let container = try session.apfsContainer(forMainPartitionOf: wholeDisk)
            let dataDevice = try dataVolumeDevice(inContainer: container, session: session)
            _ = try session.run("/usr/sbin/diskutil", ["mount", dataDevice])
            let mountPoint = try mountPoint(forDevice: dataDevice, session: session)
            return MountedDataVolume(session: session, mountPoint: mountPoint)
        } catch {
            session.unmountDiskIfMounted()
            session.detach()
            throw error
        }
    }

    private func dataVolumeDevice(inContainer container: String, session: DiskImageSession) throws -> String {
        let plist = try session.runPlist("/usr/sbin/diskutil", ["apfs", "list", "-plist", container])
        guard let containers = plist["Containers"] as? [[String: Any]] else {
            throw UnattendedError.commandExecutionFailed("Could not parse APFS container list for \(container)")
        }

        for container in containers {
            guard let volumes = container["Volumes"] as? [[String: Any]] else { continue }
            for volume in volumes {
                let roles = volume["Roles"] as? [String] ?? []
                if roles.contains("Data"), let device = volume["DeviceIdentifier"] as? String {
                    return device
                }
            }
        }

        throw UnattendedError.commandExecutionFailed("Could not find APFS Data volume in \(container)")
    }

    private func mountPoint(forDevice device: String, session: DiskImageSession) throws -> URL {
        let plist = try session.runPlist("/usr/sbin/diskutil", ["info", "-plist", device])
        guard let mountPoint = plist["MountPoint"] as? String, !mountPoint.isEmpty else {
            throw UnattendedError.commandExecutionFailed("Could not determine mount point for \(device)")
        }
        return URL(fileURLWithPath: mountPoint, isDirectory: true)
    }

    private func cleanup(mount: MountedDataVolume) {
        _ = try? mount.session.run("/usr/sbin/diskutil", ["unmount", mount.mountPoint.path])
        mount.session.detach()
    }

    // MARK: - Guest Patching

    private func patchMountedDataVolume(at mountPoint: URL) throws {
        let user = GuestUser(username: "lume", realName: "lume", password: "lume", uid: "501", gid: "20")

        try ensureDirectoryExecutable(mountPoint.appendingPath("private/var/db/dslocal/nodes/Default"))
        try ensureDirectoryExecutable(mountPoint.appendingPath("private/var/db/dslocal/nodes/Default/users"))

        let userUUID = try createOrUpdateUser(user, mountPoint: mountPoint)
        try addUserToGroups(user, uuid: userUUID, mountPoint: mountPoint)
        try createHomeAndUserPreferences(user, mountPoint: mountPoint)
        try markSetupAssistantComplete(user, mountPoint: mountPoint)
        try configureAutologin(user, mountPoint: mountPoint)
        try enableSSH(mountPoint: mountPoint)
        try configurePowerAndLockSettings(mountPoint: mountPoint)
    }

    private struct GuestUser {
        let username: String
        let realName: String
        let password: String
        let uid: String
        let gid: String

        var homeDirectory: String { "/Users/\(username)" }
    }

    private func createOrUpdateUser(_ user: GuestUser, mountPoint: URL) throws -> String {
        let usersDir = mountPoint.appendingPath("private/var/db/dslocal/nodes/Default/users")
        let userPlist = usersDir.appendingPathComponent("\(user.username).plist")

        var record: [String: Any]
        let userUUID: String

        if fileManager.fileExists(atPath: userPlist.path) {
            record = try readPlist(userPlist)
            userUUID = (record["generateduid"] as? [String])?.first ?? UUID().uuidString.uppercased()
        } else {
            try ensureUIDAvailable(user.uid, usersDir: usersDir)
            userUUID = UUID().uuidString.uppercased()
            record = [
                "_writers_UserCertificate": [user.username],
                "_writers_hint": [user.username],
                "_writers_jpegphoto": [user.username],
                "_writers_passwd": [user.username],
                "_writers_picture": [user.username],
                "_writers_realname": [user.username],
                "record_daemon_version": ["9040000"],
                "unlockOptions": ["0"]
            ]
        }

        record["ShadowHashData"] = [try shadowHashData(for: user.password)]
        record["authentication_authority"] = [";ShadowHash;HASHLIST:<SALTED-SHA512-PBKDF2>"]
        record["generateduid"] = [userUUID]
        record["gid"] = [user.gid]
        record["home"] = [user.homeDirectory]
        record["name"] = [user.username]
        record["passwd"] = ["********"]
        record["realname"] = [user.realName]
        record["shell"] = ["/bin/zsh"]
        record["uid"] = [user.uid]

        try writePlist(record, to: userPlist, mode: 0o600, preserveExisting: fileManager.fileExists(atPath: userPlist.path))
        return userUUID
    }

    private func ensureUIDAvailable(_ uid: String, usersDir: URL) throws {
        guard let enumerator = fileManager.enumerator(at: usersDir, includingPropertiesForKeys: nil) else {
            throw UnattendedError.commandExecutionFailed("Could not enumerate \(usersDir.path)")
        }

        for case let url as URL in enumerator where url.pathExtension == "plist" {
            guard let record = try? readPlist(url),
                  let existingUID = (record["uid"] as? [String])?.first,
                  existingUID == uid else {
                continue
            }
            throw UnattendedError.commandExecutionFailed("UID \(uid) already exists in \(url.lastPathComponent)")
        }
    }

    private func addUserToGroups(_ user: GuestUser, uuid: String, mountPoint: URL) throws {
        let groupsDir = mountPoint.appendingPath("private/var/db/dslocal/nodes/Default/groups")
        for groupName in ["admin", "staff"] {
            let groupPlist = groupsDir.appendingPathComponent("\(groupName).plist")
            guard fileManager.fileExists(atPath: groupPlist.path) else { continue }

            var group = try readPlist(groupPlist)
            var users = group["users"] as? [String] ?? []
            if !users.contains(user.username) {
                users.append(user.username)
            }
            group["users"] = users

            var members = group["groupmembers"] as? [String] ?? []
            if !members.contains(uuid) {
                members.append(uuid)
            }
            group["groupmembers"] = members

            try writePlist(group, to: groupPlist, mode: permissions(of: groupPlist) ?? 0o644, preserveExisting: true)
        }
    }

    private func createHomeAndUserPreferences(_ user: GuestUser, mountPoint: URL) throws {
        let home = mountPoint.appendingPath(user.homeDirectory.trimmingCharacters(in: CharacterSet(charactersIn: "/")))
        for relativePath in [
            "Library/Preferences",
            "Library/Preferences/ByHost",
            "Desktop",
            "Documents",
            "Downloads"
        ] {
            try createDirectory(home.appendingPath(relativePath), mode: 0o755)
        }
        try setPermissions(home, 0o755)

        let textEncoding = String(format: "0x%X:0x0:0x0\n", Int(user.uid) ?? 501)
        try writeData(Data(textEncoding.utf8), to: home.appendingPath(".CFUserTextEncoding"), mode: 0o644)

        var globalPreferences = try readPlistIfPresent(home.appendingPath("Library/Preferences/.GlobalPreferences.plist"))
        globalPreferences["AppleKeyboardUIMode"] = 3
        try writePlist(globalPreferences, to: home.appendingPath("Library/Preferences/.GlobalPreferences.plist"))

        var screensaver = try readPlistIfPresent(home.appendingPath("Library/Preferences/com.apple.screensaver.plist"))
        screensaver["askForPassword"] = 0
        screensaver["askForPasswordDelay"] = 0
        screensaver["idleTime"] = 0
        try writePlist(screensaver, to: home.appendingPath("Library/Preferences/com.apple.screensaver.plist"))

        for uuid in byHostUUIDs(in: home.appendingPath("Library/Preferences/ByHost")) {
            try writePlist(
                ["idleTime": 0],
                to: home.appendingPath("Library/Preferences/ByHost/com.apple.screensaver.\(uuid).plist")
            )
        }
    }

    private func markSetupAssistantComplete(_ user: GuestUser, mountPoint: URL) throws {
        let marker = mountPoint.appendingPath("private/var/db/.AppleSetupDone")
        try createDirectory(marker.deletingLastPathComponent(), mode: 0o755)
        if !fileManager.fileExists(atPath: marker.path) {
            fileManager.createFile(atPath: marker.path, contents: Data())
        }
        try setPermissions(marker, 0o644)

        let setupPreferences: [String: Any] = [
            "DidSeeCloudSetup": true,
            "DidSeePrivacy": true,
            "DidSeeSiriSetup": true,
            "DidSeeTouchIDSetup": true,
            "DidSeeTrueToneSetup": true,
            "GestureMovieSeen": "none",
            "LastSeenBuddyBuildVersion": "25F84",
            "LastSeenCloudProductVersion": "26.5.2"
        ]

        let home = mountPoint.appendingPath(user.homeDirectory.trimmingCharacters(in: CharacterSet(charactersIn: "/")))
        try writePlist(setupPreferences, to: home.appendingPath("Library/Preferences/com.apple.SetupAssistant.plist"))
        let systemSetupAssistant = mountPoint.appendingPath("Library/Preferences/com.apple.SetupAssistant.plist")
        try writePlist(
            setupPreferences,
            to: systemSetupAssistant,
            preserveExisting: fileManager.fileExists(atPath: systemSetupAssistant.path)
        )
    }

    private func configureAutologin(_ user: GuestUser, mountPoint: URL) throws {
        let loginwindowPath = mountPoint.appendingPath("Library/Preferences/com.apple.loginwindow.plist")
        var loginwindow = try readPlistIfPresent(loginwindowPath)

        var accountInfo = loginwindow["AccountInfo"] as? [String: Any] ?? [:]
        // A FirstLogins entry causes macOS to launch Setup Assistant on the
        // next graphical login, even when .AppleSetupDone is present.
        accountInfo.removeValue(forKey: "FirstLogins")
        accountInfo["MaximumUsers"] = max(accountInfo["MaximumUsers"] as? Int ?? 1, 1)
        accountInfo["OnConsole"] = accountInfo["OnConsole"] as? [String: Any] ?? [:]

        loginwindow["AccountInfo"] = accountInfo
        loginwindow["autoLoginUser"] = user.username
        loginwindow["GuestEnabled"] = false
        loginwindow["lastUser"] = "loggedIn"
        loginwindow["lastUserName"] = user.username
        loginwindow["RecentUsers"] = [user.username]
        loginwindow["SHOWFULLNAME"] = false

        try writePlist(
            loginwindow,
            to: loginwindowPath,
            preserveExisting: fileManager.fileExists(atPath: loginwindowPath.path)
        )

        let kcpassword = mountPoint.appendingPath("private/etc/kcpassword")
        try createDirectory(kcpassword.deletingLastPathComponent(), mode: 0o755)
        try writeData(kcpasswordData(for: user.password), to: kcpassword, mode: 0o600, preserveExisting: fileManager.fileExists(atPath: kcpassword.path))
    }

    private func enableSSH(mountPoint: URL) throws {
        let disabledPlist = mountPoint.appendingPath("private/var/db/com.apple.xpc.launchd/disabled.plist")
        let launchdStateDir = disabledPlist.deletingLastPathComponent()
        try createDirectory(launchdStateDir, mode: 0o755)

        let migratedMarker = launchdStateDir.appendingPathComponent("disabled.migrated")
        if !fileManager.fileExists(atPath: migratedMarker.path) {
            fileManager.createFile(atPath: migratedMarker.path, contents: Data())
        }
        try setPermissions(migratedMarker, 0o644)

        var disabled = try readPlistIfPresent(disabledPlist)
        disabled["com.openssh.sshd"] = false
        try writePlist(
            disabled,
            to: disabledPlist,
            preserveExisting: fileManager.fileExists(atPath: disabledPlist.path),
            format: .xml
        )
    }

    private func configurePowerAndLockSettings(mountPoint: URL) throws {
        let systemGlobal = mountPoint.appendingPath("Library/Preferences/.GlobalPreferences.plist")
        var global = try readPlistIfPresent(systemGlobal)
        global["com.apple.autologout.AutoLogOutDelay"] = 0
        try writePlist(global, to: systemGlobal, preserveExisting: fileManager.fileExists(atPath: systemGlobal.path))

        let powerManagement: [String: Any] = [
            "AC Power": [
                "Automatic Restart On Power Loss": true,
                "DarkWakeBackgroundTasks": 0,
                "Disk Sleep Timer": 0,
                "Display Sleep Timer": 0,
                "System Sleep Timer": 0,
                "Wake On LAN": 1
            ],
            "SystemPowerSettings": [
                "Update DarkWakeBG Setting": true
            ]
        ]
        let powerPath = mountPoint.appendingPath("Library/Preferences/com.apple.PowerManagement.plist")
        try writePlist(powerManagement, to: powerPath, preserveExisting: fileManager.fileExists(atPath: powerPath.path))

        let byHostDir = mountPoint.appendingPath("Users/lume/Library/Preferences/ByHost")
        for uuid in byHostUUIDs(in: byHostDir) {
            let perHostPower: [String: Any] = [
                "AC Power": [
                    "PrioritizeNetworkReachabilityOverSleep": 0,
                    "Sleep On Power Button": false,
                    "SleepServices": 0,
                    "Standby Delay": 0,
                    "Standby Enabled": 0,
                    "TCPKeepAlivePref": 1,
                    "TTYSPreventSleep": 1
                ]
            ]
            let perHostPath = mountPoint.appendingPath("Library/Preferences/com.apple.PowerManagement.\(uuid).plist")
            try writePlist(perHostPower, to: perHostPath, preserveExisting: fileManager.fileExists(atPath: perHostPath.path))
        }
    }

    // MARK: - Hashing

    private func shadowHashData(for password: String) throws -> Data {
        var salt = Data()
        salt.reserveCapacity(32)
        var generator = SystemRandomNumberGenerator()
        for _ in 0..<32 {
            salt.append(UInt8.random(in: UInt8.min...UInt8.max, using: &generator))
        }

        let entropy = pbkdf2SHA512(password: Data(password.utf8), salt: salt, iterations: 50_000, keyLength: 128)
        let shadow: [String: Any] = [
            "SALTED-SHA512-PBKDF2": [
                "entropy": entropy,
                "iterations": 50_000,
                "salt": salt
            ]
        ]
        return try PropertyListSerialization.data(fromPropertyList: shadow, format: .binary, options: 0)
    }

    private func pbkdf2SHA512(password: Data, salt: Data, iterations: Int, keyLength: Int) -> Data {
        let hmacKey = SymmetricKey(data: password)
        let hashLength = 64
        let blockCount = Int(ceil(Double(keyLength) / Double(hashLength)))
        var derivedKey = Data()

        for blockIndex in 1...blockCount {
            var blockSalt = salt
            blockSalt.append(UInt8((blockIndex >> 24) & 0xff))
            blockSalt.append(UInt8((blockIndex >> 16) & 0xff))
            blockSalt.append(UInt8((blockIndex >> 8) & 0xff))
            blockSalt.append(UInt8(blockIndex & 0xff))

            var u = Data(HMAC<SHA512>.authenticationCode(for: blockSalt, using: hmacKey))
            var t = u

            if iterations > 1 {
                for _ in 2...iterations {
                    u = Data(HMAC<SHA512>.authenticationCode(for: u, using: hmacKey))
                    for index in 0..<t.count {
                        t[index] ^= u[index]
                    }
                }
            }

            derivedKey.append(t)
        }

        return derivedKey.prefix(keyLength)
    }

    private func kcpasswordData(for password: String) -> Data {
        let key: [UInt8] = [0x7d, 0x89, 0x52, 0x23, 0xd2, 0xbc, 0xdd, 0xea, 0xa3, 0xb9, 0x1f]
        var bytes = [UInt8]()
        for (index, byte) in password.utf8.enumerated() {
            bytes.append(byte ^ key[index % key.count])
        }

        let remainder = bytes.count % 12
        if remainder != 0 {
            bytes.append(contentsOf: Array(repeating: 0, count: 12 - remainder))
        }

        return Data(bytes)
    }

    // MARK: - Plist and File Helpers

    private func readPlistIfPresent(_ url: URL) throws -> [String: Any] {
        guard fileManager.fileExists(atPath: url.path) else { return [:] }
        return try readPlist(url)
    }

    private func readPlist(_ url: URL) throws -> [String: Any] {
        let data = try Data(contentsOf: url)
        guard let plist = try PropertyListSerialization.propertyList(from: data, options: [], format: nil) as? [String: Any] else {
            throw UnattendedError.commandExecutionFailed("Invalid plist at \(url.path)")
        }
        return plist
    }

    private func writePlist(
        _ plist: [String: Any],
        to url: URL,
        mode: Int = 0o644,
        preserveExisting: Bool = false,
        format: PropertyListSerialization.PropertyListFormat = .binary
    ) throws {
        let data = try PropertyListSerialization.data(fromPropertyList: plist, format: format, options: 0)
        try writeData(data, to: url, mode: mode, preserveExisting: preserveExisting)
    }

    private func writeData(
        _ data: Data,
        to url: URL,
        mode: Int,
        preserveExisting: Bool = false
    ) throws {
        try createDirectory(url.deletingLastPathComponent(), mode: 0o755)
        let exists = fileManager.fileExists(atPath: url.path)
        if exists {
            try setPermissions(url, mode | 0o200)
        }

        if preserveExisting && exists {
            let handle = try FileHandle(forWritingTo: url)
            defer { try? handle.close() }
            try handle.truncate(atOffset: 0)
            try handle.write(contentsOf: data)
        } else {
            try data.write(to: url)
        }

        try setPermissions(url, mode)
    }

    private func createDirectory(_ url: URL, mode: Int) throws {
        try fileManager.createDirectory(at: url, withIntermediateDirectories: true)
        try setPermissions(url, mode)
    }

    private func ensureDirectoryExecutable(_ url: URL) throws {
        guard fileManager.fileExists(atPath: url.path) else { return }
        let current = permissions(of: url) ?? 0o700
        try setPermissions(url, current | 0o100)
    }

    private func setPermissions(_ url: URL, _ mode: Int) throws {
        try fileManager.setAttributes([.posixPermissions: NSNumber(value: mode)], ofItemAtPath: url.path)
    }

    private func permissions(of url: URL) -> Int? {
        guard let attrs = try? fileManager.attributesOfItem(atPath: url.path),
              let permissions = attrs[.posixPermissions] as? NSNumber else {
            return nil
        }
        return permissions.intValue
    }

    private func byHostUUIDs(in directory: URL) -> [String] {
        guard let urls = try? fileManager.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil) else {
            return []
        }

        let pattern = #"([0-9A-Fa-f]{8}-(?:[0-9A-Fa-f]{4}-){3}[0-9A-Fa-f]{12})\.plist$"#
        guard let regex = try? NSRegularExpression(pattern: pattern) else { return [] }

        return Array(Swift.Set(urls.compactMap { url in
            let name = url.lastPathComponent
            let range = NSRange(name.startIndex..<name.endIndex, in: name)
            guard let match = regex.firstMatch(in: name, range: range),
                  let uuidRange = Range(match.range(at: 1), in: name) else {
                return nil
            }
            return String(name[uuidRange]).uppercased()
        })).sorted()
    }

}

private extension URL {
    func appendingPath(_ path: String) -> URL {
        var result = self
        for component in path.split(separator: "/") where !component.isEmpty {
            result.appendPathComponent(String(component))
        }
        return result
    }
}
