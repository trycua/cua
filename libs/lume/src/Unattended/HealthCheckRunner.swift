import Foundation

/// Runs health checks to verify unattended setup success
struct HealthCheckRunner {

    /// Run a health check against a VM
    /// - Parameters:
    ///   - check: The health check configuration
    ///   - vmIP: The IP address of the VM
    /// - Returns: True if the health check passed
    func run(check: HealthCheck, vmIP: String) async throws -> Bool {
        switch check.type.lowercased() {
        case "ssh":
            return try await runSSHCheck(check: check, vmIP: vmIP)
        default:
            Logger.error("Unknown health check type", metadata: ["type": check.type])
            return false
        }
    }

    /// Run an SSH health check
    private func runSSHCheck(check: HealthCheck, vmIP: String) async throws -> Bool {
        let user = check.user ?? "lume"
        let password = check.password ?? "lume"
        let timeout = check.timeout ?? 30
        let retries = check.retries ?? 3
        let retryDelay = check.retryDelay ?? 5

        Logger.info("Running SSH health check", metadata: [
            "host": vmIP,
            "user": user,
            "timeout": "\(timeout)s",
            "retries": "\(retries)"
        ])

        for attempt in 1...retries {
            Logger.info("SSH health check attempt", metadata: ["attempt": "\(attempt)/\(retries)"])

            // Use sshpass to provide password non-interactively
            // Try to run a simple command (echo) to verify SSH works
            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
            process.arguments = [
                "sshpass", "-p", password,
                "ssh",
                "-o", "StrictHostKeyChecking=no",
                "-o", "UserKnownHostsFile=/dev/null",
                "-o", "ConnectTimeout=\(timeout)",
                "-o", "BatchMode=no",
                "\(user)@\(vmIP)",
                "echo", "health_check_ok"
            ]

            let outputPipe = Pipe()
            let errorPipe = Pipe()
            process.standardOutput = outputPipe
            process.standardError = errorPipe

            do {
                try process.run()

                // Wait for process with timeout
                let deadline = Date().addingTimeInterval(Double(timeout))
                while process.isRunning && Date() < deadline {
                    try await Task.sleep(nanoseconds: 100_000_000) // 0.1s
                }

                if process.isRunning {
                    process.terminate()
                    Logger.info("SSH health check timed out", metadata: ["attempt": "\(attempt)"])
                } else if process.terminationStatus == 0 {
                    let outputData = outputPipe.fileHandleForReading.readDataToEndOfFile()
                    let output = String(data: outputData, encoding: .utf8) ?? ""

                    if output.contains("health_check_ok") {
                        Logger.info("SSH health check passed", metadata: [
                            "host": vmIP,
                            "attempt": "\(attempt)"
                        ])
                        return true
                    }
                } else {
                    let errorData = errorPipe.fileHandleForReading.readDataToEndOfFile()
                    let errorOutput = String(data: errorData, encoding: .utf8) ?? ""
                    Logger.debug("SSH health check failed", metadata: [
                        "attempt": "\(attempt)",
                        "exitCode": "\(process.terminationStatus)",
                        "error": errorOutput.prefix(200).description
                    ])
                }
            } catch {
                Logger.debug("SSH health check error", metadata: [
                    "attempt": "\(attempt)",
                    "error": error.localizedDescription
                ])
            }

            // Wait before retry (unless this is the last attempt)
            if attempt < retries {
                Logger.info("Waiting before retry", metadata: ["delay": "\(retryDelay)s"])
                try await Task.sleep(nanoseconds: UInt64(retryDelay) * 1_000_000_000)
            }
        }

        Logger.error("SSH health check failed after all retries", metadata: [
            "host": vmIP,
            "retries": "\(retries)"
        ])
        return false
    }

    /// Run post-setup commands via SSH
    /// These are more reliable than typing commands via VNC
    func runPostSshCommands(commands: [String], vmIP: String, user: String, password: String) async throws {
        Logger.info("Running post-SSH commands", metadata: [
            "host": vmIP,
            "user": user,
            "commandCount": "\(commands.count)"
        ])

        for (index, command) in commands.enumerated() {
            Logger.info("Executing SSH command", metadata: [
                "index": "\(index + 1)/\(commands.count)",
                "command": command.prefix(50).description
            ])

            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
            process.arguments = [
                "sshpass", "-p", password,
                "ssh",
                "-o", "StrictHostKeyChecking=no",
                "-o", "UserKnownHostsFile=/dev/null",
                "-o", "ConnectTimeout=30",
                "-o", "BatchMode=no",
                "\(user)@\(vmIP)",
                command
            ]

            let outputPipe = Pipe()
            let errorPipe = Pipe()
            process.standardOutput = outputPipe
            process.standardError = errorPipe

            do {
                try process.run()
                process.waitUntilExit()

                if process.terminationStatus != 0 {
                    let errorData = errorPipe.fileHandleForReading.readDataToEndOfFile()
                    let errorOutput = String(data: errorData, encoding: .utf8) ?? ""
                    Logger.info("SSH command returned non-zero exit code", metadata: [
                        "command": command.prefix(50).description,
                        "exitCode": "\(process.terminationStatus)",
                        "error": errorOutput.prefix(200).description
                    ])
                    // Continue with other commands even if one fails
                }
            } catch {
                Logger.info("SSH command failed", metadata: [
                    "command": command.prefix(50).description,
                    "error": error.localizedDescription
                ])
                // Continue with other commands even if one fails
            }

            // Small delay between commands
            try await Task.sleep(nanoseconds: 500_000_000) // 0.5s
        }

        Logger.info("Post-SSH commands completed")
    }
}
