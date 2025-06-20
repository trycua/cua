import { describe, expect, it } from "vitest";
import * as InterfaceExports from "../../src/interface/index.ts";

describe("Interface Module Exports", () => {
  it("should export InterfaceFactory", () => {
    expect(InterfaceExports.InterfaceFactory).toBeDefined();
    expect(InterfaceExports.InterfaceFactory.createInterfaceForOS).toBeDefined();
  });

  it("should export BaseComputerInterface", () => {
    expect(InterfaceExports.BaseComputerInterface).toBeDefined();
  });

  it("should export MacOSComputerInterface", () => {
    expect(InterfaceExports.MacOSComputerInterface).toBeDefined();
  });

  it("should export LinuxComputerInterface", () => {
    expect(InterfaceExports.LinuxComputerInterface).toBeDefined();
  });

  it("should export WindowsComputerInterface", () => {
    expect(InterfaceExports.WindowsComputerInterface).toBeDefined();
  });

  it("should export all expected interfaces", () => {
    const expectedExports = [
      "InterfaceFactory",
      "BaseComputerInterface",
      "MacOSComputerInterface",
      "LinuxComputerInterface",
      "WindowsComputerInterface",
    ];

    const actualExports = Object.keys(InterfaceExports);
    for (const exportName of expectedExports) {
      expect(actualExports).toContain(exportName);
    }
  });
});
