// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { createDemoAdapter } from "@/bridge/adapters/demo";
import { noCore } from "@/bridge/__tests__/testCore";
import { BridgeProvider, useSpaces, type DataAdapter } from "@/bridge";

import { SpacesNotice } from "./spaces-notice";

afterEach(cleanup);

// The SwiftUI app's DiscoveryTests: a failed read keeps the rows and says so above the page.
describe("<SpacesNotice>", () => {
  it("shows the host's line while its list reads fail, keeps the rows, and goes with the next good read", async () => {
    const demo = createDemoAdapter({ latencyMs: 1 });
    let notice: string | null = null;
    const adapter: DataAdapter = { ...demo, listNotice: () => notice };
    const seen: { rows?: number; refresh?: () => Promise<void> } = {};
    function Rows() {
      const spaces = useSpaces();
      seen.rows = spaces.data?.length;
      seen.refresh = spaces.refresh;
      return null;
    }
    render(
      <BridgeProvider adapter={adapter} core={noCore}>
        <Rows />
        <SpacesNotice />
      </BridgeProvider>,
    );
    await waitFor(() => expect(seen.rows).toBeGreaterThan(0));
    const rows = seen.rows;
    expect(document.querySelector("[data-spaces-discovery-error]")).toBeNull();

    notice = "Could not refresh Spaces. Previously loaded rows may be out of date.";
    await act(() => seen.refresh!());
    expect(screen.getByRole("status").textContent).toBe(notice);
    expect(seen.rows).toBe(rows);

    notice = null;
    await act(() => seen.refresh!());
    expect(document.querySelector("[data-spaces-discovery-error]")).toBeNull();
  });
});
