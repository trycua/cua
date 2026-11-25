// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { afterEach, describe, expect, it } from "vitest";

import { ENGLISH_FILES } from "./english-files";
import { changeLocale, currentLocale, NAMESPACES, t, tMsg } from "./index";

afterEach(() => changeLocale("en"));

describe("t", () => {
  it("finds a key in its area's file and fills ICU variables", () => {
    expect(t("settings.general.language")).toBe("Language");
    expect(t("settings.general.language_system", { language: "English" })).toBe("System (English)");
    expect(tMsg({ code: "spaces.status.stopped_reason", vars: { reason: "no disk" } })).toBe("Stopped: no disk");
  });

  it("shows an unknown key as it is", () => {
    expect(t("nowhere.at.all")).toBe("nowhere.at.all");
  });

  it("switches to pseudo-localized English and back, live", async () => {
    await changeLocale("pseudo");
    expect(currentLocale()).toBe("pseudo");
    expect(t("settings.general.language")).toMatch(/^\[Ļáñĝûáĝé ·+\]$/);
    expect(document.documentElement.lang).toBe("en-XA");
    await changeLocale("en");
    expect(t("settings.general.language")).toBe("Language");
    expect(document.documentElement.lang).toBe("en");
  });
});

describe("english-files", () => {
  it("lists every English catalog file, for code that runs outside Vite", () => {
    expect(
      Object.keys(ENGLISH_FILES)
        .map((p) => /([^/]+)\.json$/.exec(p)![1])
        .sort(),
    ).toEqual([...NAMESPACES]);
  });
});
