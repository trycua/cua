// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// A Space's detail as this device sees it (the core's `sidebar::detail_for`,
// as the SwiftUI app's `AppModel.detail` draws it): a machine on the relay
// while this device is not enrolled, a machine that keeps its desktop
// private, and Share only with Settings, Experiments, Sharing on.

import { act, cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import type { MachineRow } from "@/bridge/contracts/host";
import { mountApp, stubBrowserApis, type TestHost } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

const NOTICE = {
  kind: "needs-enrollment",
  status: "Not enrolled",
  text: "Enroll this Mac to connect to your other machines.",
  actionLabel: "Enroll This Mac…",
};

/** One of your machines on the relay, answering with `features`. */
const machine = (features: string[]) => ({ id: "relay:studio", name: "Studio", provider: "relay", os: "macos", spacesdVersion: "0.6.0", features, reachable: true });
/** A Space Studio provides. */
const hosted = { id: "relay:studio/space-1", name: "Build box", provider: "relay", os: "linux", spacesdVersion: "0.6.0", features: ["desktop_stream"], reachable: true, host: "studio", hostName: "Studio" };

/** `over` is read on every answer: a test may change it while the page is open. */
function host(rows: unknown[], over: { access?: boolean; sharing?: boolean } = {}): TestHost {
  return {
    failsCreates: false,
    listsGhost: false,
    answer: (op, out) => {
      if (op === "spaces.list") return [...(out as unknown[]), ...rows];
      if (op === "machines.list" && over.access) return (out as MachineRow[]).map((m) => (m.current ? { ...m, accessNotice: NOTICE } : m));
      if (op === "experiments.get") return { ...(out as object), sharing: Boolean(over.sharing) };
      return out;
    },
  };
}

describe.skipIf(!wasmBuilt)("a Space's detail as this device sees it (real routes, wasm core)", () => {
  it("greys out a machine's Connect, Teleport and Share while this device is not enrolled, and its action opens the enroll sheet", async () => {
    const { router, hooks } = await mountApp("/spaces", host([machine(["desktop_stream", "window_stream", "host_spaces", "relay_attach"])], { access: true, sharing: true }));
    await waitFor(() => expect(hooks.machines.data?.find((m) => m.current)?.accessNotice).toEqual(NOTICE));
    await act(() => router.navigate({ to: "/spaces/$spaceId", params: { spaceId: "relay:studio" } }));

    await screen.findByText(NOTICE.text);
    expect((screen.getByRole("button", { name: "Connect" }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: /Teleport an app/ }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: /Share/ }) as HTMLButtonElement).disabled).toBe(true);

    fireEvent.click(screen.getByRole("button", { name: NOTICE.actionLabel }));
    await waitFor(() => expect(router.state.location.pathname).toBe("/settings/devices"));
    await waitFor(() => expect(document.querySelector("[data-enroll]")).not.toBeNull());
  });

  it("follows an approval in the open detail: the notice goes and Connect is no longer greyed out, without reopening it", async () => {
    // The SwiftUI app's DetailApprovalTests: the relay approves this device while the detail is open.
    const over = { access: true };
    const { router, hooks, emit } = await mountApp("/spaces", host([machine(["desktop_stream", "window_stream", "host_spaces", "relay_attach"])], over));
    await waitFor(() => expect(hooks.machines.data?.find((m) => m.current)?.accessNotice).toEqual(NOTICE));
    await act(() => router.navigate({ to: "/spaces/$spaceId", params: { spaceId: "relay:studio" } }));
    await screen.findByText(NOTICE.text);

    over.access = false;
    // What the host sends after its devices read (Electron's minute tick, Check again).
    act(() => emit({ type: "machines.changed" }));
    await waitFor(() => expect(screen.queryByText(NOTICE.text)).toBeNull());
    expect(decodeURIComponent(router.state.location.pathname)).toBe("/spaces/relay:studio");
    expect(screen.queryByRole("button", { name: NOTICE.actionLabel })).toBeNull();
    expect(document.querySelector('[data-desktop-cover="connect"] button[disabled]')).toBeNull();
  });

  it("shows why a machine keeps its desktop private, lists its Spaces, and offers no New Space button beside the note", async () => {
    const { router, hooks } = await mountApp("/spaces", host([machine(["host_spaces", "files"]), hosted]));
    await waitFor(() => expect(hooks.spaces.data?.some((s) => s.id === hosted.id)).toBe(true));
    await act(() => router.navigate({ to: "/spaces/$spaceId", params: { spaceId: "relay:studio" } }));

    await screen.findByText("Studio isn’t sharing its desktop. You can still create Spaces on it.");
    expect(document.querySelector("[data-stream-surface]")).toBeNull();
    const list = document.querySelector("[data-machine-spaces]")!;
    expect(list.textContent).toContain("Build box");
    expect(screen.queryByRole("button", { name: /New Space on/ })).toBeNull();
    expect(screen.queryByRole("button", { name: /Teleport an app/ })).toBeNull();
  });

  it("shows Share only while Settings, Experiments, Sharing is on", async () => {
    const off = await mountApp("/spaces/local:design-review", host([]));
    await off.router.load();
    await screen.findByRole("button", { name: /^Teleport an app$/ });
    expect(screen.queryByRole("button", { name: /^Share$/ })).toBeNull();
    cleanup();

    const on = await mountApp("/spaces/local:design-review", host([], { sharing: true }));
    await on.router.load();
    await screen.findByRole("button", { name: /^Share$/ });
  });
});
