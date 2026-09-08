import ArgumentParser
import Foundation

enum GuestPowerAction: Sendable {
  case shutdown
  case restart

  var commandName: String {
    switch self {
    case .shutdown: return "shutdown"
    case .restart: return "restart"
    }
  }

  var shutdownFlag: String {
    switch self {
    case .shutdown: return "-h"
    case .restart: return "-r"
    }
  }

  func remoteCommand(password: String) -> String {
    let escapedPassword = password.replacingOccurrences(of: "'", with: "'\\''")
    return
      "printf '%s\\n' '\(escapedPassword)' | sudo -S -p '' /bin/sh -c 'nohup /bin/sh -c \"sleep 1; /sbin/shutdown \(shutdownFlag) now\" >/dev/null 2>&1 &'"
  }
}

@MainActor
private func requestGuestPowerAction(
  _ action: GuestPowerAction,
  name: String,
  user: String,
  password: String,
  storage: String?,
  timeout: Int
) async throws {
  let controller = LumeController()
  let details: VMDetails
  do {
    details = try controller.getDetails(name: name, storage: storage)
  } catch {
    throw SSHError.vmNotFound(name)
  }

  guard details.status == "running" else {
    throw SSHError.vmNotRunning(name)
  }
  guard let ipAddress = details.ipAddress, !ipAddress.isEmpty else {
    throw SSHError.noIPAddress(name)
  }
  guard details.sshAvailable == true else {
    throw SSHError.sshNotAvailable(name)
  }

  let result = try await executeGuestPowerCommand(
    action.remoteCommand(password: password),
    host: ipAddress,
    user: user,
    password: password,
    timeout: timeout
  )
  guard result.exitCode == 0 else {
    throw SSHError.commandFailed(exitCode: result.exitCode, message: result.output)
  }

  print("Graceful \(action.commandName) requested for '\(name)'.")
}

@MainActor
private func executeGuestPowerCommand(
  _ command: String,
  host: String,
  user: String,
  password: String,
  timeout: Int
) async throws -> SSHResult {
  for attempt in 0..<2 {
    do {
      let client = SSHClient(host: host, port: 22, user: user, password: password)
      return try await client.execute(
        command: command,
        timeout: TimeInterval(timeout)
      )
    } catch let error as SSHError {
      guard case .connectionFailed = error else { throw error }
      if attempt == 0 {
        try await Task.sleep(for: .milliseconds(500))
      }
    }
  }

  Logger.debug(
    "NIO SSH connection failed for guest power command; using system SSH",
    metadata: ["host": host]
  )
  return try SystemSSHClient(
    host: host,
    port: 22,
    user: user,
    password: password
  ).execute(command: command, timeout: TimeInterval(timeout))
}

struct Shutdown: AsyncParsableCommand {
  static let configuration = CommandConfiguration(
    commandName: "shutdown",
    abstract: "Gracefully shut down a virtual machine"
  )

  @Argument(help: "Name of the virtual machine", completion: .custom(completeVMName))
  var name: String

  @Option(name: [.short, .long], help: "SSH username (default: lume)")
  var user: String = "lume"

  @Option(name: [.short, .long], help: "SSH and sudo password (default: lume)")
  var password: String = "lume"

  @Option(name: .customLong("storage"), help: "VM storage location to use")
  var storage: String?

  @Option(name: [.short, .long], help: "SSH command timeout in seconds")
  var timeout: Int = 30

  @MainActor
  func run() async throws {
    TelemetryClient.shared.record(event: "lume_shutdown")
    try await requestGuestPowerAction(
      .shutdown,
      name: name,
      user: user,
      password: password,
      storage: storage,
      timeout: timeout
    )
  }
}

struct Restart: AsyncParsableCommand {
  static let configuration = CommandConfiguration(
    commandName: "restart",
    abstract: "Gracefully restart a virtual machine"
  )

  @Argument(help: "Name of the virtual machine", completion: .custom(completeVMName))
  var name: String

  @Option(name: [.short, .long], help: "SSH username (default: lume)")
  var user: String = "lume"

  @Option(name: [.short, .long], help: "SSH and sudo password (default: lume)")
  var password: String = "lume"

  @Option(name: .customLong("storage"), help: "VM storage location to use")
  var storage: String?

  @Option(name: [.short, .long], help: "SSH command timeout in seconds")
  var timeout: Int = 30

  @MainActor
  func run() async throws {
    TelemetryClient.shared.record(event: "lume_restart")
    try await requestGuestPowerAction(
      .restart,
      name: name,
      user: user,
      password: password,
      storage: storage,
      timeout: timeout
    )
  }
}
