import { describe, expect, it } from "vitest";
import { LinuxComputerInterface } from "../../src/interface/linux.ts";
import { MacOSComputerInterface } from "../../src/interface/macos.ts";

describe("LinuxComputerInterface", () => {
  const testParams = {
    ipAddress: "test.cua.com", // TEST-NET-1 address (RFC 5737) - guaranteed not to be routable
    username: "testuser",
    password: "testpass",
    apiKey: "test-api-key",
    vmName: "test-vm",
  };

  describe("Inheritance", () => {
    it("should extend MacOSComputerInterface", () => {
      const linuxInterface = new LinuxComputerInterface(
        testParams.ipAddress,
        testParams.username,
        testParams.password,
        testParams.apiKey,
        testParams.vmName
      );

      expect(linuxInterface).toBeInstanceOf(MacOSComputerInterface);
      expect(linuxInterface).toBeInstanceOf(LinuxComputerInterface);
    });
  });
});
