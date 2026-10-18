// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { BridgeProvider, type HostAccessRow, type HostPanelView, type HostSetupFormView } from "@/bridge";
import { HostError, type DataAdapter } from "@/bridge/adapter";
import { createDemoAdapter } from "@/bridge/adapters/demo";
import { unavailableCore, type CoreClient } from "@/bridge/core";
import { ThisMachineStore, type HostFormState } from "@/bridge/this-machine";
import { HostSetupForm, logLineParts, ThisMachine } from "@/components/this-machine";

afterEach(cleanup);

const rows = (n: number, what: string): HostAccessRow[] => Array.from({ length: n }, (_, i) => ({ text: `${what} ${i + 1}`, atMs: Date.now() - i * 60_000 }));

/** The core's "This machine" page for a machine sharing over the relay, with long logs. */
const sharing: HostPanelView = {
  title: "This machine",
  summary: "Sharing · Relay",
  configured: true,
  facts: [{ label: "Name", value: "Studio" }],
  clients: [],
  clientsTitle: "Connected now",
  clientsEmpty: "Nobody",
  recentTitle: "Recent access",
  recent: rows(5, "bob@example.com viewed the desktop"),
  recentMore: "Show All…",
  recentAll: rows(8, "bob@example.com viewed the desktop"),
  recentWithBackground: [...rows(8, "bob@example.com viewed the desktop"), ...rows(3, "status check")],
  activityTitle: "Spaces activity",
  activity: rows(5, "Created QA"),
  activityMore: "Show All…",
  activityAll: rows(7, "Created QA"),
  permissions: [],
  openSettingsLabel: "Open Settings",
  actions: [
    { id: "stop-sharing", label: "Stop Sharing", destructive: false },
    { id: "remove", label: "Remove host setup", destructive: true, confirm: { title: "Remove?", message: "", confirmLabel: "Remove", cancelLabel: "Cancel" } },
  ],
};

/** Relay sharing paused while signed out (the core's words). */
const paused: HostPanelView = {
  ...sharing,
  summary: "Paused · signed out",
  notice: "Sign in to share this Mac through Cua. Sharing is paused while you’re signed out.",
  noticeAction: { id: "sign-in", label: "Sign In", destructive: false, enabled: true },
  recent: [],
  recentMore: null,
  activity: [],
  activityMore: null,
  actions: [
    { id: "remove", label: "Remove host setup", destructive: true, confirm: null },
    { id: "resume-sharing", label: "Resume Sharing", destructive: false, enabled: false, help: "Turn on a setting to share first" },
  ],
};

/** The demo host, recording `host.action` and failing it when asked. */
function host(fail?: () => Error) {
  const base = createDemoAdapter({ latencyMs: 0, signedIn: true, onboarded: true });
  const actions: string[] = [];
  const adapter = Object.assign(Object.create(base) as DataAdapter, {
    call: ((op: string, args: Record<string, unknown>) => {
      if (op === "host.action") {
        actions.push(String(args.action));
        const e = fail?.();
        if (e) return Promise.reject(e);
        return Promise.resolve({ configured: true, sharing: true, service: { installed: true, running: true, kind: "launchd" }, clients: [], permissions: [] });
      }
      return (base.call as (o: string, a: unknown) => Promise<unknown>).call(base, op, args);
    }) as DataAdapter["call"],
  });
  return { adapter, actions };
}

/** Renders the panel once the bridge's store is up (its buttons need it). */
async function mount(panel: HostPanelView, adapter: DataAdapter, progress?: string) {
  render(
    <BridgeProvider adapter={adapter} core={unavailableCore("test: no core")}>
      <ThisMachine panel={panel} progress={progress} />
    </BridgeProvider>,
  );
  await act(() => new Promise((r) => setTimeout(r, 10)));
}

describe("This machine's logs", () => {
  it("shows the newest five, and every row in Show All, with background activity on request", async () => {
    await mount(sharing, host().adapter);
    expect(document.querySelectorAll("[data-host-more]")).toHaveLength(2);
    expect(screen.getAllByText(/viewed the desktop/)).toHaveLength(5);

    fireEvent.click(document.querySelector('[data-host-more="access"]')!);
    const sheet = await waitFor(() => {
      const d = document.querySelector<HTMLElement>("[data-host-log]");
      if (!d) throw new Error("no sheet");
      return d;
    });
    expect(within(sheet).getByText("Recent access")).toBeTruthy();
    expect(within(sheet).getAllByText(/viewed the desktop/)).toHaveLength(8);
    expect(within(sheet).queryByText(/status check/)).toBeNull();
    fireEvent.click(sheet.querySelector("[data-host-log-background]")!);
    await waitFor(() => expect(within(sheet).getAllByText(/status check/)).toHaveLength(3));
    fireEvent.click(within(sheet).getByText("Done"));
    await waitFor(() => expect(document.querySelector("[data-host-log]")).toBeNull());

    // The Spaces activity in full has no background switch.
    fireEvent.click(document.querySelector('[data-host-more="activity"]')!);
    const activity = await waitFor(() => {
      const d = document.querySelector<HTMLElement>("[data-host-log]");
      if (!d) throw new Error("no sheet");
      return d;
    });
    expect(within(activity).getAllByText(/Created QA/)).toHaveLength(7);
    expect(activity.querySelector("[data-host-log-background]")).toBeNull();
  });

  it("keeps Stop Sharing and Remove in a bar under the page", async () => {
    await mount(sharing, host().adapter);
    const bar = document.querySelector("[data-host-actions]")!;
    expect(bar.className).toContain("sticky");
    expect([...bar.querySelectorAll("[data-host-action]")].map((b) => b.textContent)).toEqual(["Stop Sharing", "Remove host setup"]);
  });
});

describe("relay sharing paused while signed out", () => {
  it("says so in place of the summary, with Sign In, and what the sign-in waits for", async () => {
    const { adapter, actions } = host();
    await mount(paused, adapter, "Sign in to Cua in your browser to continue. Setup finishes on its own after that.");
    expect(document.querySelector("[data-host-summary]")).toBeNull();
    expect(document.querySelector("[data-host-notice]")?.textContent).toBe(paused.notice);
    expect(document.querySelector("[data-host-progress]")?.textContent).toMatch(/Sign in to Cua in your browser/);
    fireEvent.click(document.querySelector('[data-host-notice-action="sign-in"]')!);
    await waitFor(() => expect(actions).toEqual(["sign-in"]));
  });

  it("draws a button the core turned off as disabled, with its help as the tooltip", async () => {
    await mount(paused, host().adapter);
    const resume = document.querySelector<HTMLButtonElement>('[data-host-action="resume-sharing"]')!;
    expect(resume.disabled).toBe(true);
    expect(resume.parentElement?.getAttribute("title")).toBe("Turn on a setting to share first");
    expect(document.querySelector<HTMLButtonElement>('[data-host-action="remove"]')!.disabled).toBe(false);
  });
});

describe("a failed This machine button", () => {
  it("shows the host's words, Retry runs the same button, and Details has the raw error with Copy", async () => {
    let failing = true;
    const { adapter, actions } = host(() =>
      failing
        ? new HostError("macOS didn’t start the service your other devices connect to.", "failed", {
            title: "Couldn’t start the Cua host service",
            details: "launchctl bootstrap failed: 5",
            actionLabel: "Retry",
          })
        : (undefined as unknown as Error),
    );
    const writeText = vi.fn(async () => {});
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    await mount(sharing, adapter);
    fireEvent.click(document.querySelector('[data-host-action="stop-sharing"]')!);
    const failure = await waitFor(() => {
      const f = document.querySelector<HTMLElement>("[data-host-failure]");
      if (!f) throw new Error("no failure");
      return f;
    });
    expect(failure.querySelector("[data-host-failure-title]")?.textContent).toBe("Couldn’t start the Cua host service");
    expect(failure.querySelector("[data-host-failure-message]")?.textContent).toMatch(/didn’t start the service/);
    expect(failure.querySelector("[data-host-details-text]")).toBeNull();
    fireEvent.click(failure.querySelector("[data-host-details]")!);
    expect(failure.querySelector("[data-host-details-text]")?.textContent).toBe("launchctl bootstrap failed: 5");
    fireEvent.click(failure.querySelector("[data-host-copy-details]")!);
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("launchctl bootstrap failed: 5"));

    failing = false;
    fireEvent.click(failure.querySelector("[data-host-retry]")!);
    await waitFor(() => expect(actions).toEqual(["stop-sharing", "stop-sharing"]));
    await waitFor(() => expect(document.querySelector("[data-host-failure]")).toBeNull());
  });
});

/* ---- Host setup --------------------------------------------------------------------- */

const form: HostSetupFormView = {
  title: "Set Up for Access",
  lede: "Your other devices reach this Mac through the Cua relay.",
  fields: [],
  advancedLabel: "Advanced",
  advancedOpen: false,
  backLabel: "Back",
  submitLabel: "Set Up",
  canSubmit: true,
  busy: false,
};

describe("a failed host setup", () => {
  it("shows the title, what to do, the account's Sign In, and Details; Retry sends it again", () => {
    const retry = vi.fn();
    render(
      <HostSetupForm
        view={form}
        host={{ send: () => {}, submit: async () => false, closeForm: () => {} }}
        failure={{ title: "Sign in to Cua", message: "Your Cua account isn’t signed in on this Mac. Sign in, then try again.", details: "Not signed in to Cua", actionLabel: "Sign In" }}
        onRetry={retry}
      />,
    );
    expect(document.querySelector("[data-host-failure-title]")?.textContent).toBe("Sign in to Cua");
    const button = document.querySelector<HTMLButtonElement>("[data-host-retry]")!;
    expect(button.textContent).toBe("Sign In");
    fireEvent.click(button);
    expect(retry).toHaveBeenCalledTimes(1);
    expect(document.querySelector("[data-host-details]")).toBeTruthy();
  });

  it("says what a running setup waits for, and Retrying… while it runs", () => {
    render(
      <HostSetupForm
        view={{ ...form, busy: true }}
        host={{ send: () => {}, submit: async () => false, closeForm: () => {} }}
        failure={{ title: "Couldn’t connect", message: "Cua Spaces couldn’t reach the internet.", details: "connection refused" }}
        progress="Sign in to Cua in your browser to continue. Setup finishes on its own after that."
        onRetry={() => {}}
      />,
    );
    expect(document.querySelector("[data-host-progress]")?.textContent).toMatch(/Sign in to Cua in your browser/);
    const button = document.querySelector<HTMLButtonElement>("[data-host-retry]")!;
    expect(button.textContent).toBe("Retrying…");
    expect(button.disabled).toBe(true);
  });
});

/** The core's form calls, enough for the store: the form is valid and
 * builds this request; `submit` and `failed` set `busy`. */
function formCore(): CoreClient {
  const request = { mode: "relay", name: "Studio" };
  return {
    status: "ready",
    methods: [],
    call: () => {
      throw new Error("unused");
    },
    tryCall: <T,>(method: string, args?: Record<string, unknown>) => {
      const state = (args?.state ?? {}) as HostFormState;
      const action = args?.action as { type: string; error?: string } | undefined;
      switch (method) {
        case "host.formInitial":
          return { name: "Studio", allow: "", advanced: false, direct: false, listen: "", relayUrl: "", busy: false, error: null, spare: false } as T;
        case "host.formReduce":
          return { ...state, busy: action?.type === "submit", error: action?.type === "failed" ? action.error : state.error } as T;
        case "host.formView":
          return { ...form, busy: state.busy, error: state.error, request } as T;
      }
      return undefined;
    },
  };
}

describe("Retry on a failed setup", () => {
  it("sends the same request again and closes the form once it worked", async () => {
    const sent: unknown[] = [];
    let fail = true;
    const adapter = {
      call: async (op: string, args: { request: unknown }) => {
        if (op !== "host.setUp") return null;
        sent.push(args.request);
        if (fail) throw new HostError("Cua Spaces couldn’t reach the internet.", "failed", { title: "Couldn’t connect", details: "connection refused", actionLabel: "Retry" });
        return null;
      },
    } as unknown as DataAdapter;
    const store = new ThisMachineStore(adapter, formCore(), async () => {});
    store.openForm();
    expect(await store.submit(null)).toBe(false);
    expect(store.get().setupError).toEqual({ title: "Couldn’t connect", message: "Cua Spaces couldn’t reach the internet.", details: "connection refused", actionLabel: "Retry" });
    // The form keeps the raw error.
    expect(store.get().form?.error).toBe("connection refused");

    fail = false;
    await act(async () => {
      expect(await store.retrySetUp()).toBe(true);
    });
    expect(sent).toEqual([
      { mode: "relay", name: "Studio" },
      { mode: "relay", name: "Studio" },
    ]);
    expect(store.get()).toMatchObject({ form: null, setupError: null });
  });
});

describe("a long log line", () => {
  it("is cut in the middle: who gives way, what happened stays in view", async () => {
    expect(logLineParts("Ada (5a1c7e02-3b4d) · Screen and input refused ×108")).toEqual(["Ada (5a1c7e02-3b4d) · ", "Screen and input refused ×108"]);
    expect(logLineParts("Bob viewed thumbnails ×3")).toEqual(["Bob viewed thumbnails", " ×3"]);
    expect(logLineParts("Bob viewed thumbnails")).toEqual(["Bob viewed thumbnails", ""]);
    const long = "Ada Lovelace (5a1c7e02-3b4d-4e6f-8a9b-0c1d2e3f4a5b) · Screen and input ×108";
    await mount({ ...sharing, recent: [{ text: long, atMs: Date.now() }], recentMore: null }, host().adapter);
    const row = document.querySelector<HTMLElement>("[data-host-log-row]")!;
    expect(row.querySelector("[data-host-log-tail]")?.textContent).toBe("Screen and input ×108");
    expect(row.querySelector("[title]")?.getAttribute("title")).toBe(long);
    // The access log keeps its tail whole; who gives way.
    expect(row.querySelector("[data-host-log-tail]")?.className).toContain("shrink-0");
    expect(row.querySelector("[data-host-log-tail]")?.className).not.toContain("truncate");
    expect(row.querySelector("[data-host-log-head]")?.className).not.toContain("shrink-0");
  });

  it("keeps the Spaces activity's action and Space whole instead", async () => {
    await mount({ ...sharing, recent: [], recentMore: null, activity: [{ text: "Deleted space-e0a1 · Ada Lovelace (5a1c7e02-3b4d-4e6f)", atMs: Date.now() }], activityMore: null }, host().adapter);
    const row = document.querySelector<HTMLElement>("[data-host-log-row]")!;
    expect(row.querySelector("[data-host-log-head]")?.textContent).toBe("Deleted space-e0a1 · ");
    expect(row.querySelector("[data-host-log-head]")?.className).toContain("shrink-0");
    expect(row.querySelector("[data-host-log-tail]")?.className).not.toContain("shrink-0");
    expect(row.querySelector("[data-host-log-head]")?.className).not.toContain("truncate");
  });
});
