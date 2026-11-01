// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Settings, General: the "Repo and dependency cache" row. A switch, a size
// limit, Clear cache and what it holds in plain words.

import { act, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
afterEach(cleanup);

describe.skipIf(!wasmBuilt)("Settings, Repo and dependency cache", () => {
  it("shows what it holds in plain words and changes the switch, the size and clears", async () => {
    const calls: [string, unknown][] = [];
    await mountApp("/settings/general", {
      failsCreates: false,
      listsGhost: false,
      intercept: (op, args) => {
        if (op === "repoCache.set" || op === "repoCache.clear") calls.push([op, args]);
        return undefined;
      },
    });
    const sw = await waitFor(() => {
      const el = document.querySelector<HTMLElement>("#repo-cache");
      expect(el).not.toBeNull();
      return el!;
    });
    const text = document.body.textContent ?? "";
    expect(text).toContain("Repo and dependency cache");
    expect(document.querySelector("[data-repo-cache-summary]")?.textContent).toBe("Repos: 3, 1.2 GB of 5 GB");
    expect(document.querySelector("[data-repo-cache-week]")?.textContent).toMatch(/^Last week: \d+% of clones came from the cache, about 2 minutes saved$/);
    // Words the product avoids, and a middle dot.
    const group = document.querySelector('[data-settings-group="repo-cache"]')?.textContent ?? "";
    expect(group).not.toContain("·");
    expect(group).not.toMatch(/device|computer/i);
    // It does not promise dependencies yet.
    expect(group.replace("Repo and dependency cache", "")).not.toMatch(/dependenc|package|lockfile/i);

    // Size: empty is automatic; a number saves when the field is left.
    const size = document.querySelector<HTMLInputElement>('[data-setting-row="repo-cache-size"]')!;
    expect(size.value).toBe("");
    expect(size.placeholder).toBe("Automatic");
    await act(async () => void fireEvent.change(size, { target: { value: "2" } }));
    await act(async () => void fireEvent.blur(size));
    await waitFor(() => expect(calls).toEqual([["repoCache.set", { sizeGb: 2 }]]));
    await waitFor(() => expect(document.querySelector<HTMLInputElement>('[data-setting-row="repo-cache-size"]')!.value).toBe("2"));
    await waitFor(() => expect(document.querySelector("[data-repo-cache-summary]")?.textContent).toBe("Repos: 3, 1.2 GB of 2 GB"));
    // A number that is not valid is put back, not saved.
    await act(async () => void fireEvent.change(size, { target: { value: "-3" } }));
    await act(async () => void fireEvent.blur(size));
    expect(calls.length).toBe(1);

    // Clear cache empties it.
    const clear = document.querySelector<HTMLButtonElement>('[data-setting-row="repo-cache-clear"]')!;
    await act(async () => void fireEvent.click(clear));
    await waitFor(() => expect(calls[1]).toEqual(["repoCache.clear", {}]));
    await waitFor(() => expect(document.querySelector("[data-repo-cache-summary]")?.textContent).toBe("Repos: 0, 0 MB of 2 GB"));
    expect(document.querySelector("[data-repo-cache-week]")?.textContent).toBe("No clones yet this week");

    // Off hides the size and Clear; the switch calls the host.
    await act(async () => void fireEvent.click(sw));
    await waitFor(() => expect(calls[2]).toEqual(["repoCache.set", { enabled: false }]));
    await waitFor(() => expect(document.querySelector('[data-setting-row="repo-cache-size"]')).toBeNull());
  });
});
