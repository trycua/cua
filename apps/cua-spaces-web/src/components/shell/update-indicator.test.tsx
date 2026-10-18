// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { createDemoAdapter } from "@/bridge/adapters/demo";
import { noCore } from "@/bridge/__tests__/testCore";
import { BridgeProvider, type DataAdapter, type UpdateState } from "@/bridge";

const upsell = vi.hoisted(() => ({ open: false }));
vi.mock("./relay-allowance-notice", () => ({ useRelayAllowanceShowing: () => upsell.open }));

import { SidebarUpdateIndicator, TopBarUpdateIndicator } from "./update-indicator";

afterEach(() => {
  cleanup();
  upsell.open = false;
});

const NOTES = "https://github.com/trycua/cua/releases/tag/cua-spaces-v0.9.3";
const ready: Partial<UpdateState> = { status: "downloaded", version: "0.9.3", percent: 100, notesUrl: NOTES };

/** The demo host with an updater in `update`, recording what the page asks of it. */
function host(update: Partial<UpdateState> | undefined, opts: { quiet?: boolean } = {}) {
  const demo = createDemoAdapter({ latencyMs: 0, update });
  const calls: string[] = [];
  const opened: string[] = [];
  const adapter: DataAdapter = {
    ...demo,
    call: (op, args) => {
      calls.push(op);
      if (op === "session.openExternal") {
        opened.push((args as { url: string }).url);
        return Promise.resolve(null) as ReturnType<typeof demo.call>;
      }
      // No agent is working: a restart cuts nothing short.
      if (opts.quiet && op === "agents.runs") return Promise.resolve([]) as ReturnType<typeof demo.call>;
      return demo.call(op, args);
    },
  };
  const view = (ui = <SidebarUpdateIndicator />) =>
    render(
      <BridgeProvider adapter={adapter} core={noCore}>
        {ui}
      </BridgeProvider>,
    );
  return { demo, calls, opened, view };
}

describe("<SidebarUpdateIndicator>", () => {
  it("shows nothing while up to date", async () => {
    const { view, calls } = host(undefined);
    view();
    await waitFor(() => expect(calls).toContain("update.get"));
    await act(() => new Promise((r) => setTimeout(r, 10)));
    expect(document.querySelector("[data-update-indicator]")).toBeNull();
  });

  it("shows the download quietly, with its percent and no button", async () => {
    const { view } = host({ status: "downloading", version: "0.9.3", percent: 42 });
    view();
    const row = await screen.findByRole("status");
    expect(row.textContent).toBe("Downloading update 0.9.3 (42%)");
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("when ready, offers Restart to update and a What's new link to the release notes", async () => {
    const { view, calls, opened } = host(ready, { quiet: true });
    view();
    expect(await screen.findByText("Update ready")).toBeTruthy();
    expect(screen.getByText("Cua Spaces 0.9.3")).toBeTruthy();
    fireEvent.click(screen.getByText("What's new"));
    await waitFor(() => expect(opened).toEqual([NOTES]));
    // Nothing installed until the click.
    expect(calls).not.toContain("update.install");
    fireEvent.click(screen.getByRole("button", { name: "Restart to update" }));
    await waitFor(() => expect(calls).toContain("update.install"));
    // The demo host relaunched into 0.9.3: the indicator is gone.
    await waitFor(() => expect(document.querySelector("[data-update-indicator]")).toBeNull());
  });

  it("asks before restarting while an agent is running; Not now changes nothing, Restart anyway installs", async () => {
    const { view, calls } = host(ready);
    view();
    // The demo has a run in progress.
    await waitFor(() => expect(calls).toContain("agents.runs"));
    await act(() => new Promise((r) => setTimeout(r, 20)));
    fireEvent.click(await screen.findByRole("button", { name: "Restart to update" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog.textContent).toContain("Restarting Cua Spaces now stops");
    fireEvent.click(screen.getByRole("button", { name: "Not now" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(calls).not.toContain("update.install");
    fireEvent.click(screen.getByRole("button", { name: "Restart to update" }));
    fireEvent.click(await screen.findByRole("button", { name: "Restart anyway" }));
    await waitFor(() => expect(calls).toContain("update.install"));
  });

  it("steps back to an icon while an upgrade prompt is open", async () => {
    upsell.open = true;
    const { view } = host(ready);
    view();
    expect(await screen.findByRole("button", { name: "Update ready: restart to install" })).toBeTruthy();
    expect(screen.queryByText("Restart to update")).toBeNull();
    expect(screen.queryByText("What's new")).toBeNull();
  });

  it("shows nothing on a host with no updater (the call is unsupported)", async () => {
    const demo = createDemoAdapter({ latencyMs: 0 });
    const adapter: DataAdapter = { ...demo, call: (op, args) => (op === "update.get" ? Promise.reject(Object.assign(new Error("unsupported"), { code: "unsupported" })) : demo.call(op, args)) } as DataAdapter;
    render(
      <BridgeProvider adapter={adapter} core={noCore}>
        <SidebarUpdateIndicator />
      </BridgeProvider>,
    );
    await act(() => new Promise((r) => setTimeout(r, 10)));
    expect(document.querySelector("[data-update-indicator]")).toBeNull();
  });
});

describe("<TopBarUpdateIndicator>", () => {
  it("is one icon with the same name, for when the sidebar is closed", async () => {
    const { view } = host(ready);
    view(<TopBarUpdateIndicator />);
    expect(await screen.findByRole("button", { name: "Update ready: restart to install" })).toBeTruthy();
  });
});
