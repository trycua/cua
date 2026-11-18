// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The name this computer gets in the account's lists (the SwiftUI app's
// ComputerNameTests). Device test, 10 Nov: a hosted Mac showed as its full
// host name, cut off.
import { describe, expect, it } from "vitest";
import { personName } from "../src/model/devices";

describe("computer name", () => {
  it("keeps real names and shortens host names", () => {
    expect(personName("j17-2.macminivault.com")).toBe("j17-2");
    expect(personName("Dillons-MacBook-Pro-2.local")).toBe("Dillons-MacBook-Pro-2");
    expect(personName("Mac.localdomain")).toBe("Mac");
    expect(personName("Ada's MacBook Pro\n")).toBe("Ada's MacBook Pro");
    expect(personName("Ada's Mac v2.1")).toBe("Ada's Mac v2.1");
    expect(personName("studio")).toBe("studio");
    expect(personName("10.0.0.5")).toBe("10.0.0.5");
    expect(personName("  build.example.com  ")).toBe("build");
    expect(personName(".hidden")).toBe(".hidden");
  });
});
