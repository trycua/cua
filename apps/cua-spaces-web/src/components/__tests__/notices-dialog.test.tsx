// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("@/bridge", () => ({ useSession: () => ({ openExternal: vi.fn(async () => {}) }) }));

import { MAC_APP_NOTICES_PATH, NoticesDialog, tablesAsLists } from "@/components/settings/notices-dialog";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  delete document.documentElement.dataset.host;
});

describe("NoticesDialog (About, Acknowledgements)", () => {
  it("in the Mac app, shows the app's bundled notices (from the app, not the network) before the web UI's", async () => {
    document.documentElement.dataset.host = "macos";
    const fetch = vi.fn(async () => new Response("# Third-party notices\n\nDynamicNotchKit by Kai Azim", { status: 200 }));
    vi.stubGlobal("fetch", fetch);
    render(<NoticesDialog open onOpenChange={() => {}} />);
    await waitFor(() => expect(screen.getByText(/DynamicNotchKit by Kai Azim/)).toBeTruthy());
    expect(fetch).toHaveBeenCalledWith(MAC_APP_NOTICES_PATH);
    expect(MAC_APP_NOTICES_PATH.startsWith("/")).toBe(true);
    expect(screen.getByText("Cua Spaces for Mac")).toBeTruthy();
  });

  it("elsewhere, asks for no app file and shows the web UI's notices", async () => {
    const fetch = vi.fn();
    vi.stubGlobal("fetch", fetch);
    render(<NoticesDialog open onOpenChange={() => {}} />);
    await waitFor(() => expect(screen.queryByText("Loading…")).toBeNull());
    expect(fetch).not.toHaveBeenCalled();
    expect(screen.queryByText("Cua Spaces for Mac")).toBeNull();
  });

  it("draws a notices table as a list (the Markdown view has no tables)", () => {
    const md = "Intro\n\n| Project | Used in |\n|---|---|\n| A @ 1 | x.swift |\n| B | y.swift |\n\nAfter";
    expect(tablesAsLists(md)).toBe("Intro\n\n- A @ 1 · x.swift\n- B · y.swift\n\nAfter");
  });
});
