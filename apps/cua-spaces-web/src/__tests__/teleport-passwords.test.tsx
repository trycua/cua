// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Teleport a browser (CUA-1641): ticking "Saved passwords" in the review
// lists them to pick, as "Keep me signed in" lists its sites: by site with
// counts when reading the browser, by site and username from the Keyvault.
// No password value is ever on the page; only the picked ones are sent.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it } from "vitest";
import type { PickerEvent, PickerState, TeleportStore } from "@/bridge";
import { testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import { pickerConsent, pickerReduce, pickerReview } from "@/bridge/teleport";
import type { CoreClient } from "@/bridge/core";
import { TeleportReview } from "@/components/teleport/review";

afterEach(cleanup);

const flow = JSON.parse(
  readFileSync(join(import.meta.dirname, "../../../../libs/cua/crates/cua-spaces-app-core/parity/teleport-sign-ins.json"), "utf8"),
) as { spaceName: string; entries: unknown; plan: unknown; picker: Record<string, unknown>[] };

/** The review of Chrome with its sites read: the flow up to the first
 * `domains-loaded`. */
function reviewState(core: CoreClient): PickerState {
  let s = core.call<PickerState>("flow.initial", { spaceName: flow.spaceName });
  for (const e of flow.picker) {
    const ev = { ...e } as Record<string, unknown>;
    if (e.type === "loaded") ev.entries = flow.entries;
    if (e.type === "planned") ev.plan = flow.plan;
    s = pickerReduce(core, s, ev as unknown as PickerEvent);
    if (e.type === "domains-loaded") return s;
  }
  throw new Error("no domains-loaded in the flow");
}

function Review({ core, start, sent }: { core: CoreClient; start: PickerState; sent: PickerEvent[] }) {
  const [state, setState] = useState(start);
  const teleport = {
    send: (e: PickerEvent) => {
      sent.push(e);
      setState((s) => pickerReduce(core, s, e));
    },
    sendFrom: async (source: "live" | "vault") => setState((s) => pickerReduce(core, s, { type: "send-from", source })),
    bytes: (n: number) => `${n} B`,
  } as unknown as TeleportStore;
  return (
    <div data-consent={JSON.stringify(pickerConsent(core, state))}>
      <TeleportReview review={pickerReview(core, state)!} readingSites={false} vault={null} teleport={teleport} />
    </div>
  );
}

/** A saved password choice's checkbox, by its key. */
const box = (key: string) => document.querySelector<HTMLElement>(`[data-review-password="${key}"] [role="checkbox"]`)!;

const consentOf = () => JSON.parse(document.querySelector<HTMLElement>("[data-consent]")!.dataset.consent!) as Record<string, unknown>;

describe.skipIf(!wasmBuilt)("Teleport review, saved passwords", () => {
  it("lists the sites with saved passwords once ticked, and sends only the picked ones", async () => {
    const core = await testCore();
    const sent: PickerEvent[] = [];
    render(<Review core={core} start={reviewState(core)} sent={sent} />);
    expect(document.querySelector("[data-review-password]")).toBeNull();
    fireEvent.click(document.querySelector("[data-review-passwords]")!);
    const rows = () => [...document.querySelectorAll<HTMLElement>("[data-review-password]")].map((r) => `${r.dataset.reviewPassword}${r.dataset.selected ? " x" : ""}`);
    // Starts from the sites kept signed in.
    expect(rows()).toEqual(["bank.example", "github.com x", "google.com"]);
    expect(document.querySelector("[data-review-passwords-summary]")?.textContent).toBe("1 of 3 sites");
    fireEvent.click(box("bank.example"));
    expect(rows()).toEqual(["bank.example x", "github.com x", "google.com"]);
    expect(consentOf()).toMatchObject({ includePasswords: true, passwordDomains: ["bank.example", "github.com"] });
    // The search filters; None drops what it shows.
    fireEvent.change(screen.getByRole("textbox", { name: "Search saved passwords" }), { target: { value: "bank" } });
    expect(rows()).toEqual(["bank.example x"]);
    fireEvent.click(screen.getAllByRole("button", { name: "None" }).at(-1)!);
    expect(consentOf()).toMatchObject({ passwordDomains: ["github.com"] });
    expect(sent.map((e) => e.type)).toEqual(["toggle-passwords", "toggle-password", "password-query", "select-shown-passwords"]);
  });

  it("lists each saved password of the Keyvault by site and username, never a value", async () => {
    const core = await testCore();
    let s = reviewState(core);
    s = pickerReduce(core, s, {
      type: "vault-items",
      count: 1,
      newestMs: 1,
      nowMs: 1,
      selected: ["c1"],
      passwordIds: ["p1", "p2"],
      passwords: [
        { id: "p1", site: "github.com", username: "octocat" },
        { id: "p2", site: "google.com", username: "ada@example.test" },
      ],
    });
    s = pickerReduce(core, s, { type: "send-from", source: "vault" });
    s = pickerReduce(core, s, { type: "toggle-passwords", value: true });
    render(<Review core={core} start={s} sent={[]} />);
    expect(screen.getByText("octocat")).toBeTruthy();
    expect(screen.getByText("ada@example.test")).toBeTruthy();
    fireEvent.click(box("p2"));
    expect(consentOf()).toMatchObject({ includePasswords: true, fromVault: ["c1", "p1"] });
    expect(screen.getByText("Also send 1 saved password")).toBeTruthy();
  });
});
