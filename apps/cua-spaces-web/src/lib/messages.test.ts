// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { readFileSync, readdirSync } from "node:fs";
import { describe, expect, it } from "vitest";

import { messageText } from "./messages";

const tauri = "../cua-spaces/src/i18n/locales/en";
const web = "src/i18n/locales/en";

const catalog = (dir: string) => {
  const out: Record<string, string> = {};
  for (const f of readdirSync(dir)) Object.assign(out, JSON.parse(readFileSync(`${dir}/${f}`, "utf8")));
  return out;
};

describe("core messages", () => {
  it("say the same as the Tauri app's English for the codes both have", () => {
    const theirs = catalog(tauri);
    const ours = catalog(web);
    for (const code of ["teleport.sensitive.history.label", "spaces.status.stopped_reason", "spaces.power.turn_on"]) {
      expect(ours[code], code).toBeDefined();
      expect(ours[code], code).toBe(theirs[code]);
    }
  });

  it("fills variables and shows an unknown code as it is", () => {
    expect(messageText({ code: "spaces.status.stopped_reason", vars: { reason: "disk full" } })).toBe("Stopped: disk full");
    expect(messageText({ code: "x.unknown", vars: {} })).toBe("x.unknown");
  });
});
