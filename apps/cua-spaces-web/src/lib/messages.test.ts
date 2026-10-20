// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { readFileSync, readdirSync } from "node:fs";
import { describe, expect, it } from "vitest";

import { MESSAGES, messageText } from "./messages";

const en = "../cua-spaces/src/i18n/locales/en";

describe("core messages", () => {
  it("match the app's English catalog", () => {
    const catalog: Record<string, string> = {};
    for (const f of readdirSync(en)) Object.assign(catalog, JSON.parse(readFileSync(`${en}/${f}`, "utf8")));
    for (const [code, text] of Object.entries(MESSAGES)) expect(catalog[code], code).toBe(text);
  });

  it("fills variables and shows an unknown code as it is", () => {
    expect(messageText({ code: "x.unknown", vars: {} })).toBe("x.unknown");
  });
});
