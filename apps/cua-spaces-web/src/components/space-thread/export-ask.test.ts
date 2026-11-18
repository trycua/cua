// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";
import { exportAskText } from "./export-ask";

describe("exportAskText", () => {
  it("words a push as Push and open", () => {
    expect(exportAskText("export:repo.push", " 2 files changed ")).toEqual({
      title: "Push to GitHub",
      allow: "Push and open",
      detail: "2 files changed",
      account: null,
    });
  });
  it("covers any other export and ignores normal approvals", () => {
    expect(exportAskText("export:other", "")?.allow).toBe("Allow");
    expect(exportAskText("Push to main", "")).toBeNull();
  });

  it("names the account the host will push with, as a plain string", () => {
    const d =
      "Branch fix on acme/app from commit 0a1b2c3d. 2 files changed. Built in Space. Cua pushes it from this computer as ada, signed in with gh; the Space holds no GitHub access.";
    expect(exportAskText("export:repo.push", d)?.account).toBe("ada, signed in with gh");
    expect(exportAskText("export:repo.push", "2 files changed")?.account).toBeNull();
  });
});
