// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Keyvault model's pure parts (src/model/keyvault.ts): the broker's
// sentence, and what an unreachable broker looks like. The rest of the model
// runs on the app core in vault-native.test.ts.
import { describe, expect, it } from "vitest";
import type { KvPage } from "../src/native/generated/index";
import * as path from "node:path";
import { cuaHome } from "../src/model/daemon";
import { keyvaultClient } from "../src/model/environment";
import { appImagePage, brokerWords, unavailableOverview } from "../src/model/keyvault";

const sdkError = (message: string, tag = "InvalidArgument") => Object.assign(new Error(message), { [Symbol.for("typeName")]: "CuaError", tag });

describe("the broker's sentence", () => {
  it("drops the SDK's error kind, and the kind the broker puts in front of its own words", () => {
    expect(brokerWords(sdkError("CuaError.InvalidArgument: invalid argument: no waiting request req-9"))).toBe("no waiting request req-9");
    expect(brokerWords(sdkError("CuaError.PermissionDenied: denied: the user declined", "PermissionDenied"))).toBe("the user declined");
  });

  it("keeps a sentence whose start is not a kind", () => {
    // Capitals, digits and punctuation before the colon: part of the sentence.
    expect(brokerWords(sdkError("CuaError.Runtime: Touch ID: cancelled", "Runtime"))).toBe("Touch ID: cancelled");
    expect(brokerWords(sdkError("CuaError.Runtime: the vault is locked", "Runtime"))).toBe("the vault is locked");
  });

  it("leaves an error that is not the SDK's as it is", () => {
    expect(brokerWords(new Error("socket hang up: read"))).toBe("socket hang up: read");
    expect(brokerWords("plain")).toBe("plain");
  });
});

describe("an unreachable broker", () => {
  it("is a page state with nothing in it", () => {
    const o = unavailableOverview();
    expect(o.availability).toBe("not_running");
    expect(o.message).toContain("daemon is not running");
    expect([o.items, o.pending, o.grants, o.rules, o.deliveries, o.audit]).toEqual([[], [], [], [], [], []]);
    expect(o.namesVisible).toBe(false);
    expect(o.serverVerified).toBe(false);
  });
});

describe("the AppImage's Keyvault page", () => {
  const page = {
    ready: false,
    unavailableTitle: "Keyvault is unavailable: the Cua daemon is not signed by Cua (a development build, or a modified install)",
    message: "The process serving the Keyvault (...) is not signed by Cua",
  } as KvPage;

  it("says the AppImage can't use the Keyvault and to install the .deb, not a development build", () => {
    for (const availability of ["impostor", "not_first_party"]) {
      const p = appImagePage(page, availability);
      expect(p.unavailableTitle).toBe("The AppImage can't use the Keyvault");
      expect(p.message).toContain("Install the .deb");
      expect(`${p.unavailableTitle} ${p.message}`).not.toMatch(/development build|signed app/);
    }
  });

  it("leaves every other state as the core draws it", () => {
    expect(appImagePage(page, "not_running")).toBe(page);
    expect(appImagePage(page, "ready")).toBe(page);
  });
});

describe("the Keyvault client's home", () => {
  it("is the cua home the daemon serves it from, on Windows too (no HOME there)", () => {
    const homes: unknown[] = [];
    const native = {
      KeyvaultClient: class {
        constructor(home: string | undefined) {
          homes.push(home);
        }
      },
    } as never;
    keyvaultClient(native, cuaHome({ USERPROFILE: "C:\\Users\\ada" }));
    keyvaultClient(native, cuaHome({ CUA_HOME: "/srv/cua", HOME: "/home/ada" }));
    // Not a relative `.cua`, which read as "the Cua daemon is not running" on Windows.
    expect(homes).toEqual([path.join("C:\\Users\\ada", ".cua"), "/srv/cua"]);
  });
});
