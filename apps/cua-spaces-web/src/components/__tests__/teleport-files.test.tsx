// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useSyncExternalStore } from "react";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { createDemoAdapter } from "@/bridge/adapters/demo";
import { demoCatalog } from "@/bridge/adapters/demo/teleport";
import { testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import { TeleportStore } from "@/bridge/teleport";
import TeleportDialog from "@/components/teleport/teleport-dialog";

const hooks = vi.hoisted(() => ({ store: null as TeleportStore | null }));
vi.mock("@/bridge", async (actual) => ({
  ...(await actual<typeof import("@/bridge")>()),
  useTeleport: () => ({
    session: useSyncExternalStore(hooks.store!.subscribe, hooks.store!.getSession),
    images: new Map(),
    teleport: hooks.store,
  }),
}));

afterEach(() => {
  cleanup();
  hooks.store?.dispose();
  vi.restoreAllMocks();
});

const space = { id: "local:design-review", name: "design-review" };
const code = demoCatalog(0).find((e) => e.id === "code")!;
const review = () => screen.getByRole<HTMLButtonElement>("button", { name: "Review" });
const add = () => screen.getByRole("button", { name: "Add files or folders…" });

async function setup(pick: () => Promise<string[]>) {
  const adapter = createDemoAdapter({ latencyMs: 0, stepMs: 2 });
  const call = adapter.call.bind(adapter);
  vi.spyOn(adapter, "call").mockImplementation((method, args) => (method === "spaces.chooseFiles" ? (pick() as ReturnType<typeof call>) : call(method, args)));
  const store = new TeleportStore(adapter, await testCore());
  hooks.store = store;
  store.open(space, code);
  render(<TeleportDialog />);
  return store;
}

describe.skipIf(!wasmBuilt)("teleport file selection", () => {
  it("adds files and folders through the host picker, enables Review, and disables it after removing the last item", async () => {
    const pick = vi.fn().mockResolvedValueOnce([]).mockResolvedValueOnce(["/tmp/project", "/tmp/notes.txt"]).mockResolvedValueOnce(["/tmp/project"]);
    const store = await setup(pick);
    expect(screen.queryByRole("button", { name: "Add files or folders…" })).toBeNull();
    fireEvent.click(screen.getByRole("radio", { name: "The app with files or folders" }));
    expect(review().disabled).toBe(true);
    fireEvent.click(add());
    await waitFor(() => expect(pick).toHaveBeenCalledTimes(1));
    expect(review().disabled).toBe(true);
    fireEvent.click(add());
    await waitFor(() => expect(review().disabled).toBe(false));
    expect(screen.getByText("/tmp/project")).toBeTruthy();
    expect(screen.getByText("/tmp/notes.txt")).toBeTruthy();
    fireEvent.click(add());
    await waitFor(() => expect(pick).toHaveBeenCalledTimes(3));
    expect(store.getSession()!.state.files).toEqual(["/tmp/project", "/tmp/notes.txt"]);
    fireEvent.click(review());
    await waitFor(() => expect(store.getSession()!.state.step).toBe("consent"));
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    fireEvent.click(screen.getAllByRole("button", { name: "Remove" })[0]!);
    expect(review().disabled).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    expect(review().disabled).toBe(true);
  });

  it("ignores a file picker result after closing and reopening the teleport dialog", async () => {
    let finish!: (paths: string[]) => void;
    const store = await setup(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
    fireEvent.click(screen.getByRole("radio", { name: "The app with files or folders" }));
    fireEvent.click(add());
    act(() => {
      store.close();
      store.open(space, code);
      store.send({ type: "move", move: "app_with_files" });
    });
    await act(async () => finish(["/tmp/old-project"]));
    expect(store.getSession()!.state.files).toEqual([]);
    expect(review().disabled).toBe(true);
  });

  it("keeps the app options after a file picker failure so selection can be retried", async () => {
    const pick = vi.fn().mockRejectedValueOnce(new Error("Could not open the file picker")).mockResolvedValueOnce(["/tmp/project"]);
    const store = await setup(pick);
    fireEvent.click(screen.getByRole("radio", { name: "The app with files or folders" }));
    fireEvent.click(add());
    expect(await screen.findByText("Could not open the file picker")).toBeTruthy();
    expect(store.getSession()!.state.step).toBe("options");
    fireEvent.click(add());
    await waitFor(() => expect(review().disabled).toBe(false));
    expect(screen.queryByText("Could not open the file picker")).toBeNull();
  });
});
