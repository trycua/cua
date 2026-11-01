// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { createDemoAdapter } from "@/bridge/adapters/demo";
import { noCore } from "@/bridge/__tests__/testCore";
import { BridgeProvider, type DataAdapter, type UpdateState } from "@/bridge";

import { SidebarUpdateButton, TopBarUpdateIndicator } from "./update-indicator";

afterEach(cleanup);

const NOTES = "https://github.com/trycua/cua/releases/tag/cua-spaces-v0.9.3";
const ready: Partial<UpdateState> = { status: "downloaded", version: "0.9.3", percent: 100, notesUrl: NOTES };

/** The demo host with an updater in `update`, recording what the page asks of it. */
function host(update: Partial<UpdateState> | undefined, opts: { quiet?: boolean } = {}) {
  const demo = createDemoAdapter({ latencyMs: 0, stepMs: 0, update });
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
  const view = (ui = <SidebarUpdateButton />) =>
    render(
      <BridgeProvider adapter={adapter} core={noCore}>
        {ui}
      </BridgeProvider>,
    );
  return { demo, calls, opened, view };
}

/** Opens the ready button's popover. */
async function openReady() {
  fireEvent.click(await screen.findByRole("button", { name: "Update ready: restart to install" }));
  return screen.findByRole("dialog");
}

describe("<SidebarUpdateButton>", () => {
  it("shows nothing while up to date", async () => {
    const { view, calls } = host(undefined);
    view();
    await waitFor(() => expect(calls).toContain("update.get"));
    await act(() => new Promise((r) => setTimeout(r, 10)));
    expect(document.querySelector("[data-update-indicator]")).toBeNull();
  });

  it("is one quiet icon while downloading, named with the version and percent, with no dot", async () => {
    const { view } = host({ status: "downloading", version: "0.9.3", percent: 42 });
    view();
    const button = await screen.findByRole("button", { name: "Downloading update 0.9.3 (42%)" });
    expect(button.getAttribute("data-update-icon")).toBe("downloading");
    expect(document.querySelector("[data-update-dot]")).toBeNull();
    // No text in the sidebar: the name is the tooltip and the label.
    expect(screen.queryByText(/Downloading/)).toBeNull();
  });

  it("when ready, adds one dot; a click offers Restart to update and a What's new link", async () => {
    const { view, calls, opened } = host(ready, { quiet: true });
    view();
    await screen.findByRole("button", { name: "Update ready: restart to install" });
    expect(document.querySelectorAll("[data-update-dot]")).toHaveLength(1);
    const popover = await openReady();
    expect(popover.textContent).toContain("Cua Spaces 0.9.3");
    fireEvent.click(screen.getByText("What's new"));
    await waitFor(() => expect(opened).toEqual([NOTES]));
    // Nothing installed until the click.
    expect(calls).not.toContain("update.install");
    fireEvent.click(screen.getByRole("button", { name: "Restart to update" }));
    await waitFor(() => expect(calls).toContain("update.install"));
    // The demo host relaunched into 0.9.3: the button is gone.
    await waitFor(() => expect(document.querySelector("[data-update-indicator]")).toBeNull());
  });

  it("spins while Restart to update runs", async () => {
    const demo = createDemoAdapter({ latencyMs: 0, stepMs: 200, update: ready });
    const adapter: DataAdapter = { ...demo, call: (op, args) => (op === "agents.runs" ? (Promise.resolve([]) as ReturnType<typeof demo.call>) : demo.call(op, args)) };
    render(
      <BridgeProvider adapter={adapter} core={noCore}>
        <SidebarUpdateButton />
      </BridgeProvider>,
    );
    await openReady();
    fireEvent.click(screen.getByRole("button", { name: "Restart to update" }));
    expect(await screen.findByRole("button", { name: "Installing update 0.9.3…" })).toBeTruthy();
  });

  it("asks before restarting while an agent is running; Not now changes nothing, Restart anyway installs", async () => {
    const { view, calls } = host(ready);
    view();
    // The demo has a run in progress.
    await waitFor(() => expect(calls).toContain("agents.runs"));
    await act(() => new Promise((r) => setTimeout(r, 20)));
    await openReady();
    fireEvent.click(screen.getByRole("button", { name: "Restart to update" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog.textContent).toContain("Restarting Cua Spaces now stops");
    fireEvent.click(screen.getByRole("button", { name: "Not now" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(calls).not.toContain("update.install");
    await openReady();
    fireEvent.click(screen.getByRole("button", { name: "Restart to update" }));
    fireEvent.click(await screen.findByRole("button", { name: "Restart anyway" }));
    await waitFor(() => expect(calls).toContain("update.install"));
  });

  it("is a muted icon after a failed step, saying why", async () => {
    const { view } = host({ status: "error", message: "The network connection was lost." });
    view();
    const button = await screen.findByRole("button", { name: "Couldn't update: The network connection was lost. Cua Spaces tries again later." });
    fireEvent.click(button);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("shows nothing on a host with no updater (the call is unsupported)", async () => {
    const demo = createDemoAdapter({ latencyMs: 0 });
    const adapter: DataAdapter = { ...demo, call: (op, args) => (op === "update.get" ? Promise.reject(Object.assign(new Error("unsupported"), { code: "unsupported" })) : demo.call(op, args)) } as DataAdapter;
    render(
      <BridgeProvider adapter={adapter} core={noCore}>
        <SidebarUpdateButton />
      </BridgeProvider>,
    );
    await act(() => new Promise((r) => setTimeout(r, 10)));
    expect(document.querySelector("[data-update-indicator]")).toBeNull();
  });
});

describe("<TopBarUpdateIndicator>", () => {
  it("is the same button, for when the sidebar is closed", async () => {
    const { view } = host(ready);
    view(<TopBarUpdateIndicator />);
    expect(await screen.findByRole("button", { name: "Update ready: restart to install" })).toBeTruthy();
  });
});
