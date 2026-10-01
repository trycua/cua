import Testing

@testable import lume

@MainActor
@Test("VM run rejects a host at the active VM limit")
func runVMCapacityError() {
  #expect(
    Server.activeVMCapacityError(
      runningVMCount: Server.maxActiveVMs,
      maxVMs: Server.maxActiveVMs
    )
      == "VM start rejected: host active VM limit reached")
)
}

@MainActor
@Test("VM run capacity check allows an available slot")
func runVMCapacityAvailable() {
  #expect(Server.activeVMCapacityError(runningVMCount: 1, maxVMs: Server.maxActiveVMs) == nil)
}
