// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { SpaceAgentRun } from "../model/agents";
import type { TeleportBridge } from "../native/teleport";
import { SpaceAgentList } from "./SpaceAgentList";

function fakeTeleport(over: Partial<TeleportBridge> = {}): TeleportBridge {
  const unused = (() => {
    throw new Error("unused in this test");
  }) as never;
  return {
    isNative: true,
    manifest: unused,
    push: unused,
    listOpenWindows: async () => [],
    captureThumbnail: async () => null,
    appIcon: async () => null,
    spaceAppIcon: async () => null,
    spacePrimaryDisplay: async () => null,
    spaceAppIcons: async (_s: string, requests: unknown[]) => requests.map(() => null),
    listRemoteWindows: async () => [],
    listSpaceAgents: async () => [],
    remoteWindowThumbnail: async () => null,
    streamRemoteWindow: async () => {},
    listSpaces: async () => [],
    closePicker: async () => {},
    ...over,
  } as unknown as TeleportBridge;
}

function run(over: Partial<SpaceAgentRun> = {}): SpaceAgentRun {
  return {
    runId: "run-1",
    agent: "claude-code",
    status: "running",
    reason: "the turn's process is running",
    summary: "build the parser for the config format",
    createdAt: 10,
    phase: "working",
    turn: 1,
    ...over,
  };
}

function renderList(over: Partial<TeleportBridge> = {}, query = "") {
  return render(
    <SpaceAgentList spaceId="local:vm-1" teleport={fakeTeleport(over)} query={query} />,
  );
}

describe("SpaceAgentList", () => {
  it("carries no label of its own (the page's section title names it once)", async () => {
    renderList();
    await screen.findByText("No agents have been started in this Space.");
    expect(screen.queryByText("Agents")).toBeNull();
  });

  it("shows the agent name, a one-line prompt summary and a status", async () => {
    renderList({ listSpaceAgents: async () => [run()] });
    expect(await screen.findByText("Claude Code")).toBeInTheDocument();
    expect(screen.getByTitle("build the parser for the config format")).toBeInTheDocument();
    expect(screen.getByRole("img", { name: "Running" })).toBeInTheDocument();
  });

  it("gives each row the window rows' preview slot, so the sections match", async () => {
    const { container } = renderList({ listSpaceAgents: async () => [run()] });
    await screen.findByText("Claude Code");
    expect(container.querySelector(".swl-row-preview")).toBeTruthy();
    expect(container.querySelector(".swl-row")).toBeTruthy();
  });

  it("renders a crashed agent as crashed, never as idle", async () => {
    const { container } = renderList({
      listSpaceAgents: async () => [
        run({ status: "crashed", reason: "the agent process is gone and recorded no exit status" }),
      ],
    });
    expect(await screen.findByRole("img", { name: "Crashed" })).toBeInTheDocument();
    expect(screen.queryByRole("img", { name: "Idle" })).toBeNull();
    // and it must not be wearing the healthy dot
    expect(container.querySelector(".status-dot.status-running")).toBeNull();
  });

  it("renders an unknown status as unknown rather than rounding it to idle", async () => {
    renderList({ listSpaceAgents: async () => [run({ status: "unknown" })] });
    expect(await screen.findByRole("img", { name: "Unknown" })).toBeInTheDocument();
    expect(screen.queryByRole("img", { name: "Idle" })).toBeNull();
  });

  it("explains the status in the row's tooltip, using the shell's own reason", async () => {
    const { container } = renderList({
      listSpaceAgents: async () => [run({ status: "failed", reason: "the last turn exited with status 3" })],
    });
    await screen.findByRole("img", { name: "Failed" });
    expect(container.querySelector(".swl-row-agent")).toHaveAttribute(
      "title",
      "the last turn exited with status 3",
    );
  });

  /**
   * The load-bearing distinction. A failed listing rendered as "no agents" is a
   * confident false claim about the Space — the same class of bug as a status
   * panel calling a crashed agent idle.
   */
  it("says it could not read, NOT that there are no agents, when the listing fails", async () => {
    vi.spyOn(console, "warn").mockImplementation(() => {});
    renderList({
      listSpaceAgents: async () => {
        throw new Error("space unreachable");
      },
    });
    expect(await screen.findByText(/Could not read this Space’s agents/)).toBeInTheDocument();
    expect(screen.queryByText(/No agents have been started/)).toBeNull();
  });

  it("says there are none only when the Space actually answered with none", async () => {
    renderList({ listSpaceAgents: async () => [] });
    expect(await screen.findByText(/No agents have been started/)).toBeInTheDocument();
  });

  it("still shows a run whose record could not be read", async () => {
    renderList({
      listSpaceAgents: async () => [
        run({ agent: "", summary: "", status: "unknown", reason: "this run's record could not be read" }),
      ],
    });
    expect(await screen.findByText("Unknown agent")).toBeInTheDocument();
    expect(screen.getAllByTitle(/record could not be read/).length).toBeGreaterThan(0);
  });

  it("filters with the window's search box", async () => {
    const runs = [
      run({ runId: "a", summary: "build the parser" }),
      run({ runId: "b", agent: "openai-codex", summary: "write the docs" }),
    ];
    renderList({ listSpaceAgents: async () => runs }, "docs");
    expect(await screen.findByTitle("write the docs")).toBeInTheDocument();
    expect(screen.queryByTitle(/build the parser/)).toBeNull();
  });

  it("distinguishes 'no match' from 'none at all'", async () => {
    renderList({ listSpaceAgents: async () => [run()] }, "nothing-matches-this");
    expect(await screen.findByText("No agents match your filter.")).toBeInTheDocument();
  });

  it("re-reads so a finished agent stops claiming to be running", async () => {
    vi.useFakeTimers();
    try {
      let status: SpaceAgentRun["status"] = "running";
      const listSpaceAgents = vi.fn(async () => [run({ status })]);
      renderList({ listSpaceAgents });
      await vi.waitFor(() => expect(listSpaceAgents).toHaveBeenCalledTimes(1));
      status = "idle";
      await vi.advanceTimersByTimeAsync(4100);
      await vi.waitFor(() => expect(listSpaceAgents.mock.calls.length).toBeGreaterThan(1));
    } finally {
      vi.useRealTimers();
    }
  });

  it("stops polling when it goes away", async () => {
    vi.useFakeTimers();
    try {
      const listSpaceAgents = vi.fn(async () => [run()]);
      const { unmount } = renderList({ listSpaceAgents });
      await vi.waitFor(() => expect(listSpaceAgents).toHaveBeenCalledTimes(1));
      unmount();
      const after = listSpaceAgents.mock.calls.length;
      await vi.advanceTimersByTimeAsync(20000);
      expect(listSpaceAgents.mock.calls.length).toBe(after);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("SpaceAgentList rows are not falsely interactive", () => {
  it("renders rows as plain rows, since no action is wired to them yet", async () => {
    const { container } = renderList({ listSpaceAgents: async () => [run()] });
    await screen.findByText("Claude Code");
    expect(container.querySelector(".swl-row-agent")?.tagName).toBe("DIV");
    expect(container.querySelector("button")).toBeNull();
  });
});
