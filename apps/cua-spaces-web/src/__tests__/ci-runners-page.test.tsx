// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The CI Runners page on the real routes and the wasm core: setup until it is
// complete, the jobs table with its tabs and an opened row, the repository
// picker, the goal, and connecting GitHub (the hosted way and the token way).
// The host is the demo host; nothing here is a real GitHub account.

import { act, cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { ciTiming } from "@/bridge/ci-runners";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { mountApp, stubBrowserApis, type TestHost } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();
beforeEach(() => {
  ciTiming.connectPollMs = 20;
  ciTiming.pollMs = 60_000;
  ciTiming.busyPollMs = 60_000;
});

const host = (ci: "fresh" | "connected" | "busy", calls: [string, unknown][] = []): TestHost => ({
  failsCreates: false,
  listsGhost: false,
  demo: { ci },
  intercept: (op, args) => {
    if (op === "session.openExternal") {
      calls.push([op, args]);
      return null;
    }
    return undefined;
  },
});

const q = <T extends Element = HTMLElement>(sel: string) => document.querySelector<T>(sel);

describe.skipIf(!wasmBuilt)("the CI Runners page (real routes, wasm core)", () => {
  it("shows the overview line and the jobs, newest first, with the core's formats", async () => {
    const { router } = await mountApp("/ci", host("busy"));
    await router.load();
    await screen.findByText("CI Runners", { selector: "h1" });
    await waitFor(() => expect(q("[data-ci-job]")).not.toBeNull());
    const stats = [...document.querySelectorAll("[data-ci-stat]")].map((s) => s.textContent);
    expect(stats).toEqual(["Starts in 24 hours18", "Jobs in 7 days40", "Passed93%", "Spent in 7 days$3.19", "Budget this month$12.40 of $50"]);
    const keys = [...document.querySelectorAll("[data-ci-job]")].map((r) => r.getAttribute("data-ci-job"));
    expect(keys[0]).toBe("9912/301");
    const build = q('[data-ci-job="9911/298"]')!;
    expect(within(build).getByText("build")).toBeTruthy();
    expect(build.querySelector('[data-ci-cell="wait"]')?.textContent).toBe("6 s");
    expect(build.querySelector('[data-ci-cell="run"]')?.textContent).toBe("1 min 12 s");
    expect(build.querySelector('[data-ci-cell="cost"]')?.textContent).toBe("$0.04");
    // Setup is complete, so no checklist.
    expect(q("[data-ci-setup]")).toBeNull();
  });

  it("filters by tab and opens a row to its timeline, computer, cloud and Open on GitHub", async () => {
    const calls: [string, unknown][] = [];
    const { router } = await mountApp("/ci", host("busy", calls));
    await router.load();
    await waitFor(() => expect(q("[data-ci-job]")).not.toBeNull());
    act(() => q<HTMLElement>('[data-ci-tab="failed"]')!.click());
    await waitFor(() => expect([...document.querySelectorAll("[data-ci-job]")].map((r) => r.getAttribute("data-ci-job"))).toEqual(["9910/295", "9790/205"]));
    act(() => q<HTMLElement>('[data-ci-job="9910/295"] button')!.click());
    const detail = await waitFor(() => {
      const d = q('[data-ci-job="9910/295"] [data-ci-detail]');
      expect(d).not.toBeNull();
      return d!;
    });
    expect(detail.querySelector("[data-ci-timeline]")?.textContent).toContain("Waited for a computer 21 s");
    expect(detail.querySelector('[data-ci-fact="Cloud"]')?.textContent).toContain("AWS");
    expect(detail.querySelector('[data-ci-fact="Computer"]')?.textContent).toContain("cua-ci-4");
    act(() => q<HTMLElement>("[data-ci-open-github]")!.click());
    await waitFor(() => expect(calls).toContainEqual(["session.openExternal", { url: "https://github.com/acme/api/actions/runs/9910/job/295" }]));
    // The job's own log.
    act(() => q<HTMLElement>("[data-ci-show-log]")!.click());
    const log = await waitFor(() => {
      const l = q("[data-ci-log]");
      expect(l?.textContent).toContain("The job failed after 38 s");
      return l!;
    });
    expect(log.textContent).toContain("The Space was deleted");
  });

  it("searches the repositories and switches one off", async () => {
    const { router } = await mountApp("/ci", host("busy"));
    await router.load();
    await waitFor(() => expect(q("[data-ci-repositories]")).not.toBeNull());
    expect(q("[data-ci-repo-summary]")?.textContent).toBe("3 of 5 repositories can run jobs");
    act(() => q<HTMLElement>("[data-ci-picker-toggle]")!.click());
    await waitFor(() => expect(q("[data-ci-repo]")).not.toBeNull());
    fireEvent.change(q<HTMLInputElement>("[data-ci-repo-search]")!, { target: { value: "web" } });
    await waitFor(() => expect([...document.querySelectorAll("[data-ci-repo]")].map((r) => r.getAttribute("data-ci-repo"))).toEqual(["acme/website"]));
    expect(q('[data-ci-repo="acme/website"] [data-ci-repo-status]')?.textContent).toBe("Not allowed");
    act(() => q<HTMLElement>('[data-ci-repo="acme/website"] [role="switch"]')!.click());
    await waitFor(() => expect(q("[data-ci-repo-summary]")?.textContent).toBe("4 of 5 repositories can run jobs"));
    // Choose on GitHub and Connect GitHub account are there.
    expect(q("[data-ci-choose-github]")?.textContent).toBe("Choose on GitHub");
    expect(q("[data-ci-connect-account]")?.textContent).toBe("Connect GitHub account");
  });

  it("changes the goal and says the budget used", async () => {
    const { router } = await mountApp("/ci", host("busy"));
    await router.load();
    await waitFor(() => expect(q("[data-ci-autopilot]")).not.toBeNull());
    expect(q("[data-ci-budget]")?.textContent).toBe("$12.40 of $50 used this month");
    expect(q("[data-ci-reasons]")?.textContent).toContain("Jobs arrive on weekdays");
    fireEvent.click(within(q("[data-ci-autopilot]")!).getByText("Fastest"));
    await waitFor(() => expect(q("[data-ci-goal-note]")?.textContent).toMatch(/shortest starts/));
  });

  it("starts with setup and connects GitHub the hosted way: opens GitHub, waits, then carries on by itself", async () => {
    const calls: [string, unknown][] = [];
    const { router } = await mountApp("/ci", host("fresh", calls));
    await router.load();
    await waitFor(() => expect(q("[data-ci-setup]")).not.toBeNull());
    expect(q("[data-ci-progress]")?.textContent).toBe("0 of 4");
    act(() => q<HTMLElement>('[data-ci-setup-row="connect_github"] [data-ci-action="connect"]')!.click());
    const sheet = await waitFor(() => {
      const s = q("[data-ci-connect]");
      expect(s).not.toBeNull();
      return s!;
    });
    expect(sheet.getAttribute("data-ci-connect")).toBe("choose");
    act(() => q<HTMLElement>("[data-ci-connect-primary]")!.click());
    await waitFor(() => expect(q("[data-ci-connect]")?.getAttribute("data-ci-connect")).toBe("waiting"));
    // GitHub's page opened once, from the page.
    await waitFor(() => expect(calls.filter(([op]) => op === "session.openExternal")).toHaveLength(1));
    // It carries on by itself and ends connected: the sheet closes and the steps tick.
    await waitFor(() => expect(q("[data-ci-connect]")).toBeNull(), { timeout: 3000 });
    await waitFor(() => expect(q('[data-ci-setup-row="connect_github"]')?.getAttribute("data-state")).toBe("done"));
    expect(q('[data-ci-setup-row="connect_github"]')?.textContent).toContain("Connected to acme");
    expect(q('[data-ci-setup-row="repositories"]')?.textContent).toContain("3 repositories can run jobs");
  });

  it("connects with a token: an organization is needed, a refused token says why, a good one connects", async () => {
    const calls: [string, unknown][] = [];
    const { router } = await mountApp("/ci", host("fresh", calls));
    await router.load();
    await waitFor(() => expect(q('[data-ci-action="connect"]')).not.toBeNull());
    act(() => q<HTMLElement>('[data-ci-action="connect"]')!.click());
    await waitFor(() => expect(q("[data-ci-use-token]")).not.toBeNull());
    act(() => q<HTMLElement>("[data-ci-use-token]")!.click());
    await waitFor(() => expect(q("[data-ci-token-org]")).not.toBeNull());
    expect(q<HTMLButtonElement>("[data-ci-connect-primary]")!.disabled).toBe(true);
    fireEvent.change(q<HTMLInputElement>("[data-ci-token-org]")!, { target: { value: "acme" } });
    fireEvent.change(q<HTMLInputElement>("[data-ci-token-value]")!, { target: { value: "wrong" } });
    await waitFor(() => expect(q<HTMLButtonElement>("[data-ci-connect-primary]")!.disabled).toBe(false));
    act(() => q<HTMLElement>("[data-ci-connect-primary]")!.click());
    await waitFor(() => expect(q("[data-ci-connect-error]")?.textContent).toContain("GitHub refused the token"));
    // The token field was cleared once sent: it is never kept.
    expect(q<HTMLInputElement>("[data-ci-token-value]")!.value).toBe("");
    fireEvent.change(q<HTMLInputElement>("[data-ci-token-value]")!, { target: { value: "github_pat_good" } });
    act(() => q<HTMLElement>("[data-ci-connect-primary]")!.click());
    await waitFor(() => expect(q("[data-ci-connect]")).toBeNull());
    await waitFor(() => expect(q('[data-ci-setup-row="connect_github"]')?.getAttribute("data-state")).toBe("done"));
  });

  it("opens one help topic at a time", async () => {
    const { router } = await mountApp("/ci", host("connected"));
    await router.load();
    await waitFor(() => expect(q("[data-ci-help]")).not.toBeNull());
    act(() => q<HTMLElement>('[data-ci-help-topic="queued"] button')!.click());
    await waitFor(() => expect(q('[data-ci-help-topic="queued"]')?.textContent).toContain("A job waits at GitHub"));
    act(() => q<HTMLElement>('[data-ci-help-topic="slow"] button')!.click());
    await waitFor(() => expect(q('[data-ci-help-topic="queued"]')?.getAttribute("data-open")).toBeNull());
    expect(q('[data-ci-help-topic="slow"]')?.getAttribute("data-open")).toBe("true");
  });

  it("is a row in the sidebar", async () => {
    const { router } = await mountApp("/spaces", host("busy"));
    await router.load();
    await waitFor(() => expect(screen.getAllByText("CI Runners").length).toBeGreaterThan(0));
  });

  it("is off until the experiment is on: no sidebar row, and the page says how to turn it on", async () => {
    const off: TestHost = { failsCreates: false, listsGhost: false };
    const { router } = await mountApp("/spaces", off);
    await router.load();
    await screen.findByRole("link", { name: /^Computers/ });
    expect(screen.queryByText("CI Runners")).toBeNull();
    await act(async () => {
      await router.navigate({ to: "/ci" });
    });
    await screen.findByText("CI runners is off");
    expect(screen.getByText(/cua experiments enable ci-runners/)).toBeTruthy();
    expect(q("[data-ci-job]")).toBeNull();
  });
});
