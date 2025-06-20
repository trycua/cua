import { describe, expect, it } from "vitest";
import { WindowsComputerInterface } from "../../src/interface/windows.ts";
import { MacOSComputerInterface } from "../../src/interface/macos.ts";

describe("WindowsComputerInterface", () => {
  const testParams = {
    ipAddress: "192.0.2.1", // TEST-NET-1 address (RFC 5737) - guaranteed not to be routable
    username: "testuser",
    password: "testpass",
    apiKey: "test-api-key",
    vmName: "test-vm",
  };

  describe("Inheritance", () => {
    it("should extend MacOSComputerInterface", () => {
      const windowsInterface = new WindowsComputerInterface(
        testParams.ipAddress,
        testParams.username,
        testParams.password,
        testParams.apiKey,
        testParams.vmName
      );

      expect(windowsInterface).toBeInstanceOf(MacOSComputerInterface);
      expect(windowsInterface).toBeInstanceOf(WindowsComputerInterface);
    });
  });
});
