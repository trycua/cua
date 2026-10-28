// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// An agent that is not connected (CUA-2638): its reply says so in plain
// words with Connect and Retry, the complete error is under Show details,
// the person's own message is not marked Failed, and the member list shows
// the same, uncut.

import { act, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { DEMO_THREAD_SPACE } from "@/bridge/adapters/demo/thread";
import { mountApp, stubBrowserApis } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

const q = (s: string) => document.querySelector<HTMLElement>(s);
const qa = (s: string) => [...document.querySelectorAll<HTMLElement>(s)];
const path = `/spaces/${encodeURIComponent(DEMO_THREAD_SPACE)}`;
/** The failed agent's reply (its own row: the one with a Connect), named by what it says. */
const failureReply = (who: string) => qa("[data-entry]").find((e) => e.querySelector("[data-failure-connect]") && e.textContent?.includes(who));
const host = { failsCreates: false, listsGhost: false, demo: { threadAuthFailed: true } };

describe("an agent that is not connected", () => {
  it("answers in plain words with Connect and Retry, never as the raw error, and the person's message is not marked Failed", async () => {
    await mountApp(path, host);
    const failed = await waitFor(
      () => {
        const t = qa('[data-task-state="failed"]');
        expect(t.length).toBe(2);
        return t;
      },
      { timeout: 5000 },
    );
    const claude = failed.find((t) => t.textContent?.includes("@claude hi"))!;
    // No Working or Failed chip anywhere: the failure is the agent's own message.
    expect(claude.querySelector("[data-task-chip]")).toBeNull();
    const own = claude.querySelector(":scope > [data-entry]")!;
    expect(own.querySelector("[data-task-chip]")).toBeNull();
    // Flat: the reply is its own row after the task, not inside it.
    expect(claude.querySelector("[data-task-replies]")).toBeNull();
    const reply = failureReply("Claude")!;
    expect(reply.querySelector("[data-failure-connect]")).not.toBeNull();
    expect(reply.textContent).toContain("I can't reach Claude yet. Connect Claude, then try again.");
    expect(reply.textContent).not.toMatch(/401|authentication_error|OAuth/);
    // Connect opens Settings, Connectors on the app; Retry is there too.
    const connect = reply.querySelector<HTMLAnchorElement>("[data-failure-connect]")!;
    expect(connect.getAttribute("href")).toBe("/settings/connectors/claude-code");
    expect(reply.querySelector("[data-failure-retry]")).not.toBeNull();
    // No middle dot anywhere in what the failure draws.
    expect(claude.textContent).not.toContain("·");
  });

  it("keeps the complete error under Show details, selectable, with Copy", async () => {
    await mountApp(path, host);
    const reply = await waitFor(
      () => {
        const r = failureReply("Claude");
        expect(r).toBeTruthy();
        return r as HTMLElement;
      },
      { timeout: 5000 },
    );
    expect(reply.querySelector("[data-failure-message]")).toBeNull();
    await act(async () => {
      fireEvent.click(reply.querySelector("[data-failure-details]")!);
    });
    const text = reply.querySelector("[data-failure-message]")!.textContent!;
    // Whole: the end of the provider's message is there, not clipped.
    expect(text).toContain("API Error: 401");
    expect(text).toContain("refresh your existing token.");
    expect(reply.querySelector("[data-failure-message]")!.className).toContain("select-text");
    expect(reply.querySelector("[data-failure-copy]")).not.toBeNull();
  });

  it("says one word for a failed member in the member list, and keeps the whole error under Show details", async () => {
    await mountApp(path, host);
    const row = await waitFor(
      () => {
        const r = q("[data-member='agent:hermes']");
        expect(r?.dataset.memberStatus).toBe("failed");
        return r!;
      },
      { timeout: 5000 },
    );
    const caption = row.querySelector<HTMLElement>("[data-member-caption]")!;
    // A short word, never the raw error.
    expect(caption.textContent).toBe("Not connected");
    expect(row.textContent).not.toMatch(/401|OAuth|authentication/i);
    const details = q("[data-member-details] [data-failure-details]");
    expect(details).not.toBeNull();
    await act(async () => {
      fireEvent.click(details!);
    });
    const message = q("[data-member-details] [data-failure-message]")!;
    expect(message.className).toContain("select-text");
    expect(message.textContent!.length).toBeGreaterThan(20);
  });
});
