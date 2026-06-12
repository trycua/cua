import Foundation
import Virtualization

/// Orchestrates unattended macOS installation
@MainActor
final class UnattendedInstaller {

    /// Run agent-based setup using Claude computer-use API
    func installWithAgent(
        vm: VM,
        apiKey: String,
        model: String = "claude-sonnet-4-6",
        maxIterations: Int = 100,
        systemPrompt: String? = nil,
        vncPort: Int = 0,
        noDisplay: Bool = false,
        debug: Bool = false,
        debugDir: String? = nil
    ) async throws {
        Logger.info("Starting agent-based setup", metadata: [
            "vm": vm.name,
            "model": model,
            "maxIterations": "\(maxIterations)",
            "debug": "\(debug)"
        ])

        // Start the VM (same retry logic as preset mode)
        Logger.info("Starting VM for agent setup (background task)")
        var vmStartError: Error?
        let vmTask = Task {
            var attempts = 0
            let maxAttempts = 3
            while attempts < maxAttempts {
                do {
                    try await vm.run(
                        noDisplay: noDisplay,
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
                            try await Task.sleep(nanoseconds: 5_000_000_000)
                            continue
                        }
                    }
                    vmStartError = error
                    throw error
                }
            }
        }

        // Give the VM a moment to start
        try await Task.sleep(nanoseconds: 2_000_000_000)

        if let error = vmStartError {
            Logger.error("VM failed to start", metadata: ["error": error.localizedDescription])
            throw error
        }

        // Wait for boot (agent mode uses a fixed 60s boot wait)
        let bootWait: UInt64 = 60
        Logger.info("Waiting for VM to boot", metadata: ["bootWait": "\(bootWait)s"])
        try await Task.sleep(nanoseconds: bootWait * 1_000_000_000)

        // Connect VNC input client
        Logger.info("Connecting VNC input client for agent automation")
        try await vm.vncService.connectInputClient()

        // Get display dimensions from VNC framebuffer
        let framebufferSize = await vm.vncService.getFrameBufferSize() ?? (width: 1920, height: 1440)
        let displayWidth = Int(framebufferSize.width)
        let displayHeight = Int(framebufferSize.height)

        Logger.info("VNC framebuffer size", metadata: [
            "width": "\(displayWidth)",
            "height": "\(displayHeight)"
        ])

        // Create SSH reachability check closure
        let macAddress = vm.vmDirContext.config.macAddress
        let sshCheck: (() async -> Bool)? = macAddress != nil ? {
            guard let ip = DHCPLeaseParser.getIPAddress(forMAC: macAddress!) else {
                return false
            }
            Logger.info("Checking SSH reachability", metadata: ["ip": ip])
            let runner = HealthCheckRunner()
            let check = HealthCheck(type: "ssh", user: "lume", password: "lume", timeout: 5, retries: 1, retryDelay: 0)
            return (try? await runner.run(check: check, vmIP: ip)) ?? false
        } : nil

        // Create and run the agent
        let debugDirectory: URL? = debugDir.map { URL(fileURLWithPath: $0) }
        let agent = AgentSetupRunner(
            vncService: vm.vncService,
            apiKey: apiKey,
            model: model,
            displayWidth: displayWidth,
            displayHeight: displayHeight,
            maxIterations: maxIterations,
            debug: debug,
            debugDirectory: debugDirectory,
            sshCheck: sshCheck
        )

        do {
            try await agent.run(systemPrompt: systemPrompt)
            Logger.info("Agent setup completed successfully")

            vm.vncService.disconnectInputClient()

            // Stop the VM
            Logger.info("Agent setup finished - stopping VM")
            vmTask.cancel()
            try? await vm.stop()
            Logger.info("VM stopped", metadata: ["name": vm.name])
        } catch {
            Logger.error("Agent setup failed", metadata: ["error": error.localizedDescription])
            vm.vncService.disconnectInputClient()
            vmTask.cancel()
            try? await vm.stop()
            Logger.info("VM stopped after failure", metadata: ["name": vm.name])
            throw error
        }
    }

    /// Run unattended setup on a VM using preset commands
    func install(
        vm: VM,
        config: UnattendedConfig,
        vncPort: Int = 0,
        noDisplay: Bool = false,
        debug: Bool = false,
        debugDir: String? = nil
    ) async throws {
        Logger.info("Starting unattended installation", metadata: [
            "vm": vm.name,
            "bootWait": "\(config.bootWait)s",
            "commandCount": "\(config.bootCommands.count)",
            "noDisplay": "\(noDisplay)",
            "debug": "\(debug)",
            "debugDir": debugDir ?? "default"
        ])

        // Parse all boot commands first to catch any syntax errors
        let parser = BootCommandParser()
        let commands = try parser.parseAll(config.bootCommands)

        Logger.info("Parsed boot commands", metadata: ["count": "\(commands.count)"])

        // Start the VM in a background task (vm.run() blocks forever on success)
        // We use retry logic to handle cases where auxiliary storage is still locked
        // from a previous IPSW installation (race condition with VZVirtualMachine deallocation)
        Logger.info("Starting VM for unattended setup (background task)")
        var vmStartError: Error?
        let vmTask = Task {
            var attempts = 0
            let maxAttempts = 3
            while attempts < maxAttempts {
                do {
                    try await vm.run(
                        noDisplay: noDisplay,
                        sharedDirectories: [],
                        mount: nil,
                        vncPort: vncPort,
                        recoveryMode: false,
                        usbMassStoragePaths: []
                    )
                    return  // VM started successfully
                } catch {
                    let errorDescription = error.localizedDescription
                    // Check if this is the auxiliary storage lock error
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
                    // For other errors or after max retries, store and rethrow
                    vmStartError = error
                    throw error
                }
            }
        }

        // Give the VM a moment to start up and check for early failures
        try await Task.sleep(nanoseconds: 2_000_000_000)  // 2 seconds

        // Check if VM start failed early (e.g., auxiliary storage lock error)
        if let error = vmStartError {
            Logger.error("VM failed to start", metadata: ["error": error.localizedDescription])
            throw error
        }

        // Wait for initial boot
        Logger.info("Waiting for VM to boot", metadata: ["bootWait": "\(config.bootWait)s"])
        try await Task.sleep(nanoseconds: UInt64(config.bootWait) * 1_000_000_000)

        // Connect VNC input client for sending mouse/keyboard events
        Logger.info("Connecting VNC input client for automation")
        try await vm.vncService.connectInputClient()

        // Get display dimensions from VNC framebuffer (actual resolution)
        // This is the resolution the VNC server expects for input coordinates
        let (vncWidth, vncHeight) = await vm.vncService.getFrameBufferSize() ?? (1920, 1440)
        let displayWidth = CGFloat(vncWidth)
        let displayHeight = CGFloat(vncHeight)

        Logger.info("VNC framebuffer size", metadata: [
            "width": "\(Int(displayWidth))",
            "height": "\(Int(displayHeight))"
        ])

        // Create VNC automation engine
        let debugDirectory: URL? = debugDir.map { URL(fileURLWithPath: $0) }
        let automation = VNCAutomation(
            vncService: vm.vncService,
            displayWidth: displayWidth,
            displayHeight: displayHeight,
            debug: debug,
            debugDirectory: debugDirectory
        )

        // Execute boot commands
        Logger.info("Executing boot commands for Setup Assistant automation")

        do {
            try await automation.executeAll(commands)
            Logger.info("Unattended setup completed successfully")

            // Disconnect VNC input client (no longer needed for automation)
            vm.vncService.disconnectInputClient()

            // Run health check if configured
            if let healthCheck = config.healthCheck {
                Logger.info("Running health check", metadata: ["type": healthCheck.type])

                // Get VM IP address from MAC address
                guard let macAddress = vm.vmDirContext.config.macAddress,
                      let vmIP = DHCPLeaseParser.getIPAddress(forMAC: macAddress) else {
                    Logger.error("Cannot run health check - VM IP address not available")
                    throw UnattendedError.healthCheckFailed("VM IP address not available")
                }

                let runner = HealthCheckRunner()
                let passed = try await runner.run(check: healthCheck, vmIP: vmIP)

                if passed {
                    Logger.info("Health check passed ✓", metadata: ["type": healthCheck.type])

                    // Run post-SSH commands if configured (more reliable than VNC typing)
                    if let postCommands = config.postSshCommands, !postCommands.isEmpty {
                        Logger.info("Running post-SSH commands", metadata: ["count": "\(postCommands.count)"])
                        try await runner.runPostSshCommands(
                            commands: postCommands,
                            vmIP: vmIP,
                            user: healthCheck.user ?? "lume",
                            password: healthCheck.password ?? "lume"
                        )
                        Logger.info("Post-SSH commands completed ✓")
                    }
                } else {
                    Logger.error("Health check failed ✗", metadata: ["type": healthCheck.type])
                    throw UnattendedError.healthCheckFailed("Health check '\(healthCheck.type)' failed")
                }
            }

            // Success - stop the VM
            Logger.info("Unattended setup finished successfully - stopping VM")
            vmTask.cancel()
            try? await vm.stop()
            Logger.info("VM stopped", metadata: ["name": vm.name])
        } catch {
            Logger.error("Unattended setup failed", metadata: [
                "error": error.localizedDescription
            ])
            // Disconnect VNC input client
            vm.vncService.disconnectInputClient()
            // Cancel VM task and stop
            vmTask.cancel()
            try? await vm.stop()
            Logger.info("VM stopped after failure", metadata: ["name": vm.name])
            throw error
        }
    }
}
