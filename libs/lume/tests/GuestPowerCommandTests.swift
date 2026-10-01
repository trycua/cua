import ArgumentParser
import Testing

@testable import lume

@Test("Graceful guest power commands parse credential and storage options")
func guestPowerCommandParsing() throws {
  let shutdown = try Shutdown.parse([
    "test-vm", "--user", "admin", "--password", "secret", "--storage", "external",
    "--timeout", "45",
  ])
  let restart = try Restart.parse(["test-vm"])

  #expect(shutdown.name == "test-vm")
  #expect(shutdown.user == "admin")
  #expect(shutdown.password == "secret")
  #expect(shutdown.storage == "external")
  #expect(shutdown.timeout == 45)
  #expect(restart.name == "test-vm")
  #expect(restart.user == "lume")
  #expect(restart.password == "lume")
  #expect(restart.timeout == 30)
}

@Test("Graceful guest power commands use the correct shutdown mode and shell escaping")
func guestPowerRemoteCommands() {
  #expect(
    GuestPowerAction.shutdown.remoteCommand(password: "pa'ss")
      == "printf '%s\\n' 'pa'\\''ss' | sudo -S -p '' /bin/sh -c 'nohup /bin/sh -c \"sleep 1; /sbin/shutdown -h now\" >/dev/null 2>&1 &'"
  )
  #expect(
    GuestPowerAction.restart.remoteCommand(password: "lume")
      == "printf '%s\\n' 'lume' | sudo -S -p '' /bin/sh -c 'nohup /bin/sh -c \"sleep 1; /sbin/shutdown -r now\" >/dev/null 2>&1 &'"
  )
}
