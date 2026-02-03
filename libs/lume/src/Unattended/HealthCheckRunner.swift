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

            do {
                let sshClient = SSHClient(
                    host: vmIP,
                    port: 22,
                    user: user,
                    password: password
                )

                let result = try await sshClient.execute(
                    command: "echo health_check_ok",
                    timeout: TimeInterval(timeout)
                )

                if result.exitCode == 0 && result.output.contains("health_check_ok") {
                    Logger.info("SSH health check passed", metadata: [
                        "host": vmIP,
                        "attempt": "\(attempt)"
                    ])
                    return true
                } else {
                    Logger.debug("SSH health check failed", metadata: [
                        "attempt": "\(attempt)",
                        "exitCode": "\(result.exitCode)",
                        "output": result.output.prefix(200).description
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

        let sshClient = SSHClient(
            host: vmIP,
            port: 22,
            user: user,
            password: password
        )

        for (index, command) in commands.enumerated() {
            Logger.info("Executing SSH command", metadata: [
                "index": "\(index + 1)/\(commands.count)",
                "command": command.prefix(50).description
            ])

            // Handle sudo commands by using sudo -S which reads password from stdin
            let actualCommand: String
            if command.hasPrefix("sudo ") {
                let sudoCommand = String(command.dropFirst(5))
                let escapedPassword = password.replacingOccurrences(of: "'", with: "'\\''")
                actualCommand = "echo '\(escapedPassword)' | sudo -S \(sudoCommand)"
            } else {
                actualCommand = command
            }

            do {
                let result = try await sshClient.execute(command: actualCommand, timeout: 60)

                if result.exitCode != 0 {
                    Logger.info("SSH command returned non-zero exit code", metadata: [
                        "command": command.prefix(50).description,
                        "exitCode": "\(result.exitCode)",
                        "output": result.output.prefix(200).description
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
