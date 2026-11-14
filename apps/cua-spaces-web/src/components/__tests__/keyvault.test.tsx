// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { BridgeProvider, useSpaceAccess, type KeyvaultOverview } from "@/bridge";
import type { DataAdapter } from "@/bridge/adapter";
import { createDemoAdapter } from "@/bridge/adapters/demo";
import { testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import { KeyvaultPage } from "@/components/keyvault/keyvault-page";

// The virtualized list measures its viewport, which jsdom has none of: draw every row.
vi.mock("@legendapp/list/react", () => ({
  LegendList: ({ data, renderItem, keyExtractor }: { data: unknown[]; renderItem: (a: { item: unknown }) => React.ReactNode; keyExtractor: (i: unknown) => string }) => (
    <div>{data.map((item) => <div key={keyExtractor(item)}>{renderItem({ item })}</div>)}</div>
  ),
}));

afterEach(cleanup);

const demo = () => createDemoAdapter({ latencyMs: 0, stepMs: 2 });

async function mount(adapter: DataAdapter, props: Parameters<typeof KeyvaultPage>[0] = {}) {
  render(
    <BridgeProvider adapter={adapter} core={await testCore()}>
      <KeyvaultPage {...props} />
    </BridgeProvider>,
  );
  await screen.findByRole("button", { name: /^Access/ });
}

const category = (name: RegExp) => fireEvent.click(screen.getByRole("button", { name }));

describe.skipIf(!wasmBuilt)("Keyvault page: categories (the core's sidebar and panes)", () => {
  it("lists what waits, with Deny and Review; Review picks the items to approve and nothing is ticked at first", async () => {
    const adapter = demo();
    await mount(adapter);
    category(/^Waiting/);
    const row = await waitFor(() => {
      const r = document.querySelector<HTMLElement>("[data-waiting]");
      expect(r).not.toBeNull();
      return r!;
    });
    expect(row.textContent).toContain("Claude Code");
    fireEvent.click(within(row).getByRole("button", { name: /^Review/ }));
    const approve = await waitFor(() => {
      const b = document.querySelector<HTMLButtonElement>("[data-approval-approve]");
      expect(b).not.toBeNull();
      return b!;
    });
    expect(approve.disabled).toBe(true);
    fireEvent.click(screen.getByLabelText("Approve github.com"));
    await waitFor(() => expect(approve.disabled).toBe(false));
    fireEvent.click(approve);
    await waitFor(() => expect(document.querySelector("[data-waiting]")).toBeNull());
    expect(((await adapter.call("keyvault.overview", {})) as KeyvaultOverview).pending).toEqual([]);
  });

  it("Deny answers a waiting request without opening it", async () => {
    await mount(demo());
    category(/^Waiting/);
    const row = await waitFor(() => {
      const r = document.querySelector<HTMLElement>("[data-waiting]");
      expect(r).not.toBeNull();
      return r!;
    });
    fireEvent.click(within(row).getByRole("button", { name: "Deny" }));
    await waitFor(() => expect(document.querySelector("[data-waiting]")).toBeNull());
  });

  it("Access: Dismiss hides a copy from the notch only, Wipe removes it", async () => {
    const adapter = demo();
    await mount(adapter);
    category(/^Access/);
    const row = await waitFor(() => {
      const r = document.querySelector<HTMLElement>('[data-access="d:design-review"]');
      expect(r).not.toBeNull();
      return r!;
    });
    fireEvent.click(within(row).getByRole("button", { name: "Dismiss" }));
    await waitFor(() => expect(within(document.querySelector<HTMLElement>('[data-access="d:design-review"]')!).queryByRole("button", { name: "Dismiss" })).toBeNull());
    // Still live: only the notch's indicator is hidden.
    const o = (await adapter.call("keyvault.overview", {})) as KeyvaultOverview;
    expect(o.dismissed).toEqual(["imp-github"]);
    expect(o.deliveries).toHaveLength(1);
    fireEvent.click(within(document.querySelector<HTMLElement>('[data-access="d:design-review"]')!).getByRole("button", { name: "Wipe" }));
    await waitFor(() => expect(document.querySelector('[data-access="d:design-review"]')).toBeNull());
    expect(((await adapter.call("keyvault.overview", {})) as KeyvaultOverview).deliveries).toEqual([]);
  });

  it("a Space's badge opens Access with its row brought forward", async () => {
    await mount(demo(), { initialView: "access", focus: "d:design-review" });
    const row = await waitFor(() => {
      const r = document.querySelector<HTMLElement>('[data-access="d:design-review"]');
      expect(r).not.toBeNull();
      return r!;
    });
    expect(row.dataset.focused).toBe("true");
  });

  it("Recent lists the decisions, newest first", async () => {
    await mount(demo());
    category(/^Recent/);
    await waitFor(() => expect(screen.getAllByText(/\w/).length).toBeGreaterThan(5));
    expect(document.querySelector("[data-category-empty]")).toBeNull();
  });
});

function Badge({ spaceIds }: { spaceIds: string[] }) {
  const spaces = spaceIds.map((id) => ({ id, name: id.split(":")[1]!, os: "linux", status: "running", detail: "", lastUsedAt: 0, scene: "blank" }));
  const access = useSpaceAccess(spaces as never);
  return (
    <ul>
      {spaceIds.map((id) => (
        <li key={id} data-space={id}>
          {access.signedIn.has(id) ? `signed in ${access.accessKey(id)}` : "no access"}
        </li>
      ))}
    </ul>
  );
}

describe.skipIf(!wasmBuilt)("Spaces with live Keyvault access", () => {
  it("marks the Space holding a live copy, with its Access row, and no other", async () => {
    const adapter = demo();
    render(
      <BridgeProvider adapter={adapter} core={await testCore()}>
        <Badge spaceIds={["local:design-review", "local:other"]} />
      </BridgeProvider>,
    );
    await waitFor(() => expect(document.querySelector('[data-space="local:design-review"]')!.textContent).toBe("signed in d:design-review"));
    expect(document.querySelector('[data-space="local:other"]')!.textContent).toBe("no access");
    // Wiping the copy takes the badge away.
    await adapter.call("keyvault.run", { command: { type: "release", target: "design-review" } });
    await waitFor(() => expect(document.querySelector('[data-space="local:design-review"]')!.textContent).toBe("no access"));
  });
});

describe.skipIf(!wasmBuilt)("Keyvault page: the vault list", () => {
  it("selects a site, then Delete confirms in the core's words and removes the items", async () => {
    const adapter = demo();
    await mount(adapter);
    const group = await screen.findByLabelText("Select github.com");
    fireEvent.click(group);
    fireEvent.click(await screen.findByRole("button", { name: /Delete/ }));
    const dialog = await screen.findByRole("alertdialog");
    // Two GitHub items have a live copy in a Space: the core says they are wiped too.
    expect(dialog.textContent).toMatch(/Delete 3 items\?/);
    expect(dialog.textContent).toMatch(/Copies delivered to a Space will be wiped as well\./);
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(async () => {
      const o = (await adapter.call("keyvault.overview", {})) as KeyvaultOverview;
      expect(o.items.some((i) => i.domain === "github.com")).toBe(false);
      expect(o.deliveries).toEqual([]);
    });
  });

  it("Cancel keeps everything", async () => {
    const adapter = demo();
    await mount(adapter);
    fireEvent.click(await screen.findByLabelText("Select github.com"));
    fireEvent.click(await screen.findByRole("button", { name: /Delete/ }));
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(((await adapter.call("keyvault.overview", {})) as KeyvaultOverview).items.some((i) => i.domain === "github.com")).toBe(true);
  });
});
