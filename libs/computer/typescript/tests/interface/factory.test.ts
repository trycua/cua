import { describe, expect, it } from "vitest";
import { InterfaceFactory } from "../../src/interface/factory.ts";
import { MacOSComputerInterface } from "../../src/interface/macos.ts";
import { LinuxComputerInterface } from "../../src/interface/linux.ts";
import { WindowsComputerInterface } from "../../src/interface/windows.ts";
import { OSType } from "../../src/types.ts";

describe("InterfaceFactory", () => {
  const testParams = {
    ipAddress: "192.168.1.100",
    username: "testuser",
    password: "testpass",
    apiKey: "test-api-key",
    vmName: "test-vm",
  };

  describe("createInterfaceForOS", () => {
    it("should create MacOSComputerInterface for macOS", () => {
      const interface_ = InterfaceFactory.createInterfaceForOS(
        OSType.MACOS,
        testParams.ipAddress,
        testParams.apiKey,
        testParams.vmName
      );

      expect(interface_).toBeInstanceOf(MacOSComputerInterface);
    });

    it("should create LinuxComputerInterface for Linux", () => {
      const interface_ = InterfaceFactory.createInterfaceForOS(
        OSType.LINUX,
        testParams.ipAddress,
        testParams.apiKey,
        testParams.vmName
      );

      expect(interface_).toBeInstanceOf(LinuxComputerInterface);
    });

    it("should create WindowsComputerInterface for Windows", () => {
      const interface_ = InterfaceFactory.createInterfaceForOS(
        OSType.WINDOWS,
        testParams.ipAddress,
        testParams.apiKey,
        testParams.vmName
      );

      expect(interface_).toBeInstanceOf(WindowsComputerInterface);
    });

    it("should throw error for unsupported OS type", () => {
      expect(() => {
        InterfaceFactory.createInterfaceForOS(
          "unsupported" as OSType,
          testParams.ipAddress,
          testParams.apiKey,
          testParams.vmName
        );
      }).toThrow("Unsupported OS type: unsupported");
    });

    it("should create interface without API key and VM name", () => {
      const interface_ = InterfaceFactory.createInterfaceForOS(
        OSType.MACOS,
        testParams.ipAddress
      );

      expect(interface_).toBeInstanceOf(MacOSComputerInterface);
    });
  });
});
