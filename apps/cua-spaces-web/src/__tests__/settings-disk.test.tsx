// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Settings, Advanced, Storage: what Cua keeps, moving a folder, removing an
// image after a question, and the Uninstall question with its Keychain box.

import { act, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
afterEach(cleanup);

const click = async (el: Element | null) => {
  expect(el).not.toBeNull();
  await act(async () => void fireEvent.click(el!));
};

describe.skipIf(!wasmBuilt)("Settings, Storage", () => {
  it("shows the folders and moves one only after a question that names the size", async () => {
    const calls: unknown[] = [];
    await mountApp("/settings/advanced", {
      failsCreates: false,
      listsGhost: false,
      intercept: (op, args) => {
        if (op === "disk.do") calls.push(args);
        return undefined;
      },
    });
    const group = await waitFor(() => {
      const el = document.querySelector<HTMLElement>('[data-settings-group="disk"]');
      expect(el).not.toBeNull();
      return el!;
    });
    const text = group.textContent ?? "";
    expect(text).toContain("Cua keeps 14.2 GB on this Mac");
    expect(text).toContain("412.0 GB free");
    expect(text).toContain("Computers");
    expect(text).toContain("~/.cua/runtimes");
    expect(text).not.toContain("·");
    expect(text).not.toMatch(/\bpool\b/i);

    await click(group.querySelector('[data-setting-row="disk-change-runtimes"]'));
    // The question names the size and the folder; nothing is moved yet.
    await waitFor(() => expect(document.body.textContent).toContain("Move computers to this folder?"));
    expect(document.body.textContent).toContain("11.3 GB");
    expect(document.body.textContent).toContain("/Volumes/Fast/Cua");
    expect(calls.map((c) => (c as { op: string }).op)).toEqual(["locate", "check"]);
    await click([...document.querySelectorAll("button")].find((b) => b.textContent === "Move") ?? null);
    await waitFor(() => expect(calls.map((c) => (c as { op: string }).op)).toEqual(["locate", "check", "set"]));
    expect(calls[2]).toEqual({ op: "set", kind: "runtimes", path: "/Volumes/Fast/Cua", move: true });
    // The old copy is listed and removing it asks first.
    await waitFor(() => expect(document.querySelector('[data-setting-row="disk-remove-old"]')).not.toBeNull());
  });

  it("never removes without a confirmation, and Uninstall passes the Keychain choice", async () => {
    const calls: { op: string; keychain?: boolean }[] = [];
    await mountApp("/settings/advanced", {
      failsCreates: false,
      listsGhost: false,
      intercept: (op, args) => {
        if (op === "disk.do") calls.push(args as never);
        return undefined;
      },
    });
    await waitFor(() => expect(document.querySelector('[data-setting-row="disk-remove-item"]')).not.toBeNull());
    await click(document.querySelector('[data-setting-row="disk-remove-item"]'));
    await waitFor(() => expect(document.body.textContent).toContain("This frees"));
    expect(calls).toEqual([]);
    await click([...document.querySelectorAll("button")].find((b) => b.textContent === "Cancel") ?? null);
    expect(calls).toEqual([]);

    await click(document.querySelector('[data-setting-row="disk-uninstall"]'));
    await waitFor(() => expect(document.body.textContent).toContain("Uninstall Cua?"));
    expect(document.body.textContent).toContain("Also remove sign-in and keys from the Keychain");
    await click([...document.querySelectorAll("button")].find((b) => b.textContent === "Uninstall") ?? null);
    await waitFor(() => expect(calls).toEqual([{ op: "uninstall", keychain: false }]));
  });
});
