import Foundation

/// Orchestrates unattended macOS installation
@MainActor
final class UnattendedInstaller {
    // Publish the offline-created administrator to the VM's paired Recovery environment.
    static let recoveryUserMetadataRefreshCommand = "/usr/sbin/diskutil apfs updatePreboot / >/dev/null"

    static var guestFinalizationScript: String {
        """
        set -e
        /usr/bin/touch /var/db/.AppleSetupDone
        /usr/sbin/chown root:wheel /var/db/.AppleSetupDone
        /bin/chmod 644 /var/db/.AppleSetupDone
        /usr/bin/defaults write /Library/Preferences/com.apple.loginwindow autoLoginUser -string lume
        /usr/bin/defaults write /Library/Preferences/com.apple.loginwindow GuestEnabled -bool false
        /usr/bin/defaults write /Library/Preferences/com.apple.loginwindow lastUser -string loggedIn
        /usr/bin/defaults write /Library/Preferences/com.apple.loginwindow lastUserName -string lume
        /usr/bin/defaults write /Library/Preferences/com.apple.SetupAssistant DidSeeCloudSetup -bool true
        /usr/bin/defaults write /Library/Preferences/com.apple.SetupAssistant DidSeePrivacy -bool true
        /usr/bin/defaults write /Library/Preferences/com.apple.SetupAssistant DidSeeSiriSetup -bool true
        /usr/bin/defaults write /Library/Preferences/com.apple.SetupAssistant DidSeeTouchIDSetup -bool true
        /usr/bin/defaults write /Library/Preferences/com.apple.SetupAssistant DidSeeTrueToneSetup -bool true
        \(recoveryUserMetadataRefreshCommand)
        /bin/launchctl enable system/com.openssh.sshd
        /usr/bin/stat -f 'MARKER_OWNER=%u:%g' /var/db/.AppleSetupDone
        """
    }

    /// Run unattended setup on a VM by patching the installed macOS disk offline.
    func install(
        vm: VM,
        config: UnattendedConfig,
        vncPort: Int = 0,
        noDisplay: Bool = false,
        debug: Bool = false,
        debugDir: String? = nil
    ) async throws {
        Logger.info("Starting offline unattended installation", metadata: [
            "vm": vm.name,
            "configBootCommandCount": "\(config.bootCommands.count)",
            "postSshCommandCount": "\(config.postSshCommands?.count ?? 0)",
            "verifyVncPort": "\(vncPort)",
            "compatNoDisplayFlag": "\(noDisplay)",
            "compatDebugFlag": "\(debug)",
            "compatDebugDir": debugDir ?? "default"
        ])

        try await materializeFirstBootState(vm: vm, vncPort: vncPort)

        let patcher = MacOSOfflineSetupPatcher()
        try patcher.patch(vm: vm)

        try await verifySetup(vm: vm, config: config, vncPort: vncPort)
    }

    private func materializeFirstBootState(vm: VM, vncPort: Int) async throws {
        Logger.info("Starting VM once to materialize first-boot state before offline patch", metadata: [
            "vm": vm.name
        ])

        var vmStartError: Error?
        let vmTask = Task {
            do {
                try await vm.run(
                    noDisplay: true,
                    sharedDirectories: [],
                    mount: nil,
                    vncPort: vncPort,
                    recoveryMode: false,
                    usbMassStoragePaths: []
                )
            } catch {
                vmStartError = error
                throw error
            }
        }

        try await Task.sleep(nanoseconds: 2_000_000_000)

        if let error = vmStartError {
            Logger.error("VM failed to start during first-boot materialization", metadata: [
                "error": error.localizedDescription
            ])
            throw error
        }

        do {
            let vmIP = try await waitForIPAddress(vm: vm, timeoutSeconds: 120)
            Logger.info("First-boot state materialized enough for offline patch", metadata: [
                "vm": vm.name,
                "ip": vmIP
            ])
        } catch {
            vmTask.cancel()
            try? await vm.stop()
            throw error
        }

        try await Task.sleep(nanoseconds: 10_000_000_000)

        Logger.info("Stopping VM before offline patch", metadata: ["vm": vm.name])
        vmTask.cancel()
        try? await vm.stop()
        try await Task.sleep(nanoseconds: 3_000_000_000)
    }

    private func verifySetup(vm: VM, config: UnattendedConfig, vncPort: Int) async throws {
        Logger.info("Starting VM to verify offline unattended setup", metadata: [
            "vm": vm.name
        ])

        var vmStartError: Error?
        let vmTask = Task {
            var attempts = 0
            let maxAttempts = 3
            while attempts < maxAttempts {
                do {
                    try await vm.run(
                        noDisplay: true,
                        sharedDirectories: [],
                        mount: nil,
                        vncPort: vncPort,
                        recoveryMode: false,
                        usbMassStoragePaths: []
                    )
                    return
                } catch {
                    let errorDescription = error.localizedDescription
                    if errorDescription.contains("auxiliary storage") || errorDescription.contains("Failed to lock") {
                        attempts += 1
                        if attempts < maxAttempts {
                            Logger.info("VM start failed due to auxiliary storage lock, retrying", metadata: [
                                "attempt": "\(attempts)/\(maxAttempts)",
                                "waitTime": "5s"
                            ])
                            try await Task.sleep(nanoseconds: 5_000_000_000)  // Wait 5 seconds before retry
                            continue
                        }
                    }
                    vmStartError = error
                    throw error
                }
            }
        }

        try await Task.sleep(nanoseconds: 2_000_000_000)

        if let error = vmStartError {
            Logger.error("VM failed to start", metadata: ["error": error.localizedDescription])
            throw error
        }

        do {
            let vmIP = try await waitForIPAddress(vm: vm, timeoutSeconds: 300)
            let healthCheck = config.healthCheck ?? HealthCheck(
                type: "ssh",
                user: "lume",
                password: "lume",
                timeout: 5,
                retries: 60,
                retryDelay: 5
            )

            Logger.info("Running offline setup health check", metadata: [
                "type": healthCheck.type,
                "ip": vmIP
            ])

            let runner = HealthCheckRunner()
            let passed = try await runner.run(check: healthCheck, vmIP: vmIP)

            guard passed else {
                throw UnattendedError.healthCheckFailed("Health check '\(healthCheck.type)' failed")
            }

            try await finalizeGuestSystemSetup(
                vmIP: vmIP,
                user: healthCheck.user ?? "lume",
                password: healthCheck.password ?? "lume"
            )

            if let postCommands = config.postSshCommands, !postCommands.isEmpty {
                Logger.info("Running post-SSH commands", metadata: ["count": "\(postCommands.count)"])
                try await runner.runPostSshCommands(
                    commands: postCommands,
                    vmIP: vmIP,
                    user: healthCheck.user ?? "lume",
                    password: healthCheck.password ?? "lume"
                )
                Logger.info("Post-SSH commands completed")
            }

            Logger.info("Offline unattended setup verified - stopping VM")
            vmTask.cancel()
            try? await vm.stop()
            Logger.info("VM stopped", metadata: ["name": vm.name])
        } catch {
            Logger.error("Offline unattended setup verification failed", metadata: [
                "error": error.localizedDescription
            ])
            vmTask.cancel()
            try? await vm.stop()
            Logger.info("VM stopped after failure", metadata: ["name": vm.name])
            throw error
        }
    }

    private func finalizeGuestSystemSetup(vmIP: String, user: String, password: String) async throws {
        Logger.info("Finalizing guest system setup via SSH", metadata: [
            "host": vmIP,
            "user": user
        ])

        let sshClient = SSHClient(host: vmIP, port: 22, user: user, password: password)
        let escapedPassword = password.replacingOccurrences(of: "'", with: "'\\''")
        let script = Self.guestFinalizationScript
        let encodedScript = Data(script.utf8).base64EncodedString()
        let command = "printf '%s\\n' '\(escapedPassword)' | /usr/bin/sudo -S /bin/sh -c '/bin/echo \(encodedScript) | /usr/bin/base64 -D | /bin/sh'"
        let result = try await sshClient.execute(command: command, timeout: 60)

        guard result.exitCode == 0 else {
            throw UnattendedError.commandExecutionFailed(
                "Guest system finalization failed with exit code \(result.exitCode): \(result.output)"
            )
        }

        guard result.output.contains("MARKER_OWNER=0:0") else {
            throw UnattendedError.commandExecutionFailed(
                "Guest setup marker has unexpected ownership: \(result.output)"
            )
        }

        Logger.info("Guest system setup finalized via SSH", metadata: ["host": vmIP])
    }

    private func waitForIPAddress(vm: VM, timeoutSeconds: Int) async throws -> String {
        guard let macAddress = vm.vmDirContext.config.macAddress else {
            throw UnattendedError.healthCheckFailed("VM MAC address not available")
        }

        let deadline = Date().addingTimeInterval(TimeInterval(timeoutSeconds))
        while Date() < deadline {
            if let ip = DHCPLeaseParser.getIPAddress(forMAC: macAddress) {
                Logger.info("Found VM IP for offline setup verification", metadata: [
                    "vm": vm.name,
                    "ip": ip
                ])
                return ip
            }

            try await Task.sleep(nanoseconds: 5_000_000_000)
        }

        throw UnattendedError.healthCheckFailed("VM IP address not available after \(timeoutSeconds)s")
    }
}
