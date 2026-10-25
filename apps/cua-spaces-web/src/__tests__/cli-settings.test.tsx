// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Settings → Command line: the first run's CLI install, reachable later.

import { cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { mountApp, stubBrowserApis } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

describe.skipIf(!wasmBuilt)("Settings, Command line", () => {
  it("names the file, installs on Install, then says where it is", { timeout: 20_000 }, async () => {
    const calls: { op: string; args: unknown }[] = [];
    await mountApp("/settings", {
      failsCreates: false,
      listsGhost: false,
      demo: { cli: "missing" },
      intercept: (op, args) => {
        calls.push({ op, args });
        return undefined;
      },
    });
    const row = await waitFor(() => {
      const r = document.querySelector<HTMLElement>('[data-settings-cli="missing"]');
      expect(r).not.toBeNull();
      return r!;
    }, { timeout: 8000 });
    expect(row.textContent).toMatch(/Not installed\. Install puts it at ~\/\.local\/bin\/cua\./);
    expect(calls.some((c) => c.op === "cli.install")).toBe(false);
    fireEvent.click(row.querySelector("[data-settings-cli-install]")!);
    await waitFor(() => expect(document.querySelector('[data-settings-cli="installed"]')?.textContent).toMatch(/Installed at ~\/\.local\/bin\/cua\..*cua spaces ls/));
    expect(calls.find((c) => c.op === "cli.install")?.args).toEqual({ modifyPath: true });
  });

  it("shows where a Local build's CLI is", { timeout: 20_000 }, async () => {
    await mountApp("/settings", { failsCreates: false, listsGhost: false, demo: { cli: "local" } });
    await waitFor(() => expect(document.querySelector('[data-settings-cli="installed"]')?.textContent).toMatch(/~\/\.local\/bin\/cua-local.*cua-local spaces ls/), { timeout: 8000 });
  });
});
