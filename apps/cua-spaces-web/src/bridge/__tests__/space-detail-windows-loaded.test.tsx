// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// A home opens the first window stream that appears after the page opened. That needs to know when the Space's
// windows have been read: before the first read the list is empty because nothing is known, and a window that was
// already open must not count as new when it arrives.

import { cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { DataAdapter } from "../adapter";
import { createDemoAdapter } from "../adapters/demo";
import { BridgeProvider, useSpaceDetail, useSpaces, type SpaceDetailHook } from "../index";
import { testCore, wasmBuilt } from "./testCore";

afterEach(cleanup);

describe.skipIf(!wasmBuilt)("a Space's windows read (wasm core)", () => {
  it("says the windows are loaded only once the first read has come back", async () => {
    const base = createDemoAdapter({ latencyMs: 0, stepMs: 2 });
    let release: () => void = () => {};
    const gate = new Promise<void>((r) => (release = r));
    const adapter = Object.assign(Object.create(base) as DataAdapter, {
      call: (async (op: string, args: unknown) => {
        if (op === "spaces.windows") await gate;
        return (base.call as (o: string, a: unknown) => Promise<unknown>).call(base, op, args);
      }) as DataAdapter["call"],
    });
    const got: { detail?: SpaceDetailHook } = {};
    function Probe() {
      const spaces = useSpaces();
      const space = spaces.data?.find((s) => s.id === "local:design-review");
      got.detail = useSpaceDetail(space, null);
      return null;
    }
    render(
      <BridgeProvider adapter={adapter} core={await testCore()}>
        <Probe />
      </BridgeProvider>,
    );
    await waitFor(() => expect(got.detail?.stream).toBeTruthy());
    // The section exists (with its Desktop row) but no window read has answered: not ready to compare against.
    expect(got.detail?.windowsLoaded).toBe(false);
    release();
    await waitFor(() => expect(got.detail?.windowsLoaded).toBe(true));
  });
});
