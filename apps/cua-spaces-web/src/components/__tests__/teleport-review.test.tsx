// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { CatalogEntry, TeleportPlan, TeleportStore } from "@/bridge";
import { pickerInitial, pickerReduce, pickerReview } from "@/bridge/teleport";
import { testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import { TeleportReview } from "@/components/teleport/review";

afterEach(cleanup);

const firefox: CatalogEntry = {
  id: "firefox",
  name: "Firefox",
  hostPath: "/Applications/Firefox.app",
  hostAppId: "org.mozilla.firefox",
  version: "131.0",
  capability: "full",
  reason: null,
  moves: ["app_only", "app_with_state"],
  providerId: "firefox",
  sensitiveGroups: ["sign_ins", "history"],
  installSource: "image",
  installId: null,
  installVersion: null,
  launchBin: "firefox",
  lastUsedMs: null,
  json: "firefox",
};

describe.skipIf(!wasmBuilt)("teleport review", () => {
  // CUA-1616: "Just the app" for an app the Space's image already has
  // installs and sends nothing; the review was blank under its title.
  it("says what happens for just the app when nothing is installed or sent", async () => {
    const core = await testCore();
    let s = pickerInitial(core, "space-5973f9e636");
    s = pickerReduce(core, s, { type: "preselect", entry: firefox, files: [] });
    s = pickerReduce(core, s, { type: "move", move: "app_only" });
    s = pickerReduce(core, s, { type: "plan" });
    const plan: TeleportPlan = {
      app: firefox,
      spaceId: "local:space-5973f9e636",
      moves: "app_only",
      steps: [{ kind: "launch", summary: "Open firefox" }],
      consent: [],
      sensitive: false,
      relayUnsealed: false,
      totalBytes: 0,
      warnings: [],
      json: "{}",
    };
    s = pickerReduce(core, s, { type: "planned", plan });
    const review = pickerReview(core, s)!;
    expect(review.title).toBe("Teleport Firefox to space-5973f9e636");

    const teleport = { send: vi.fn(), sendFrom: vi.fn(), bytes: (n: number) => `${n} B` } as unknown as TeleportStore;
    const { container } = render(<TeleportReview review={review} readingSites={false} vault={null} teleport={teleport} />);
    expect(container.textContent).toContain("What happens");
    expect([...container.querySelectorAll("[data-review-step]")].map((li) => li.textContent)).toEqual(["Open firefox"]);
  });
});
