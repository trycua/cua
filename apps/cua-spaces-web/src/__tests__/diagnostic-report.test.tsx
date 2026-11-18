// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// "Send a report…" on a failed create: the review sheet shows the core's
// redacted report, each part can be edited or left out, and nothing leaves
// until the person presses Send in their mail app. Open email prefills an
// email to Cua support with only the kept parts.

import { act, cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { NAME, mountApp, stubBrowserApis } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

describe.skipIf(!wasmBuilt)("the diagnostic report for a failed create (real routes, wasm core)", () => {
  it("sends nothing until Open email, and then only the kept parts, redacted", async () => {
    const opened: string[] = [];
    const { router, hooks } = await mountApp("/spaces", {
      failsCreates: true,
      listsGhost: false,
      intercept: (op, args) => {
        if (op !== "session.openExternal") return undefined;
        opened.push((args as { url: string }).url);
        return null;
      },
    });
    await waitFor(() => expect(hooks.machines.data?.some((m) => m.id === "mac-mini")).toBe(true));
    await act(async () => {
      await hooks.spaces.createSpace({ image: "ghcr.io/trycua/macos:26", os: "macos", on: "host:mac-mini", name: NAME }).catch(() => {});
    });
    const failed = hooks.spaces.data!.find((s) => s.name === NAME && s.id.startsWith("pending:"))!;
    await act(() => router.navigate({ to: "/spaces/$spaceId", params: { spaceId: failed.id } }));

    fireEvent.click(await screen.findByRole("button", { name: "Send a report…" }));
    const error = (await screen.findByRole("textbox", { name: "Error" })) as HTMLTextAreaElement;
    expect(error.value).toContain("Local Network access is not available");
    expect((screen.getByRole("textbox", { name: "What was created" }) as HTMLTextAreaElement).value).toContain("Failure kind: local_network");
    expect(opened).toEqual([]);
    // One "Send" in the sheet's copy: the one in the mail app.
    const sheet = document.querySelector("[data-diagnostic-report]")!;
    expect(sheet.textContent).toContain("press Send there");
    expect(screen.queryByRole("button", { name: "Send" })).toBeNull();

    // Leave the app and system out, and paste a secret into the error.
    fireEvent.click(screen.getByRole("checkbox", { name: /^Include App and system/ }));
    // A fake token, split so secret scanners don't flag the source.
    const fake = "gh" + "p_abcdefghijklmnopqrstuvwxyz0123456789";
    fireEvent.change(error, { target: { value: `${error.value}\ntoken=${fake} from maya@example.com` } });
    fireEvent.click(screen.getByRole("button", { name: "Open email" }));

    await waitFor(() => expect(opened).toHaveLength(1));
    const link = opened[0]!;
    expect(link.startsWith("mailto:support@trycua.com?subject=")).toBe(true);
    expect(link).not.toContain("github");
    expect(link).not.toContain("ghp_");
    expect(link).not.toContain("maya");
    const body = decodeURIComponent(new URLSearchParams(link.split("?")[1]).get("body") ?? "");
    expect(body).toContain("Local Network access is not available");
    expect(body).not.toContain("App and system");
    expect(link.length).toBeLessThan(2100);
  });
});
