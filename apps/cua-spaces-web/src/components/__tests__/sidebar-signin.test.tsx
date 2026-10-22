// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { BridgeProvider } from "@/bridge";
import type { DataAdapter } from "@/bridge/adapter";
import { createDemoAdapter } from "@/bridge/adapters/demo";
import { unavailableCore } from "@/bridge/core";
import { AccountFooter } from "@/components/shell/sidebar";
import { ToastProvider } from "@/components/ui/toast";
import { testCore } from "@/bridge/__tests__/testCore";

afterEach(cleanup);

/** A signed-out demo host whose browser sign-in waits (as a real one does). */
function host() {
  const base = createDemoAdapter({ latencyMs: 0, signedIn: false, onboarded: true });
  const opened: string[] = [];
  const adapter = Object.assign(Object.create(base) as DataAdapter, {
    call: ((op: string, args: Record<string, unknown>) => {
      if (op === "session.signIn") {
        return Promise.resolve({ method: "device", verificationUri: "https://cua.ai/device", userCode: "WXYZ-1234" });
      }
      if (op === "session.openExternal") {
        opened.push(String(args.url));
        return Promise.resolve(null);
      }
      return (base.call as (o: string, a: unknown) => Promise<unknown>).call(base, op, args);
    }) as DataAdapter["call"],
  });
  return { adapter, opened };
}

describe("the sidebar's sign-in", () => {
  it("shows the code, the page to open again and Cancel while it waits, as the first run does", async () => {
    const { adapter, opened } = host();
    render(
      <BridgeProvider adapter={adapter} core={unavailableCore("test: no core")}>
        <AccountFooter />
      </BridgeProvider>,
    );
    fireEvent.click(await screen.findByRole("button", { name: "Sign in" }));
    const status = await screen.findByRole("status");
    expect(status.textContent).toContain("Waiting for your browser");
    await waitFor(() => expect(document.querySelector("[data-signin=waiting]")?.textContent).toContain("Code WXYZ-1234"));
    fireEvent.click(screen.getByText("Open again"));
    await waitFor(() => expect(opened).toEqual(["https://cua.ai/device"]));
    fireEvent.click(screen.getByText("Cancel"));
    expect(await screen.findByRole("button", { name: "Sign in" })).toBeTruthy();
    expect(screen.queryByText("Open again")).toBeNull();
  });
});

/** A native host (Electron, the SwiftUI app): it runs the sign-in itself,
 * answers `session.signIn` at once, and says what it waits on (the device
 * flow's code and page, on a Linux box without a browser) once it does. */
function nativeHost() {
  const base = createDemoAdapter({ latencyMs: 0, signedIn: false, onboarded: true });
  const listeners = new Set<(e: { type: string }) => void>();
  let waiting = false;
  const adapter = Object.assign(Object.create(base) as DataAdapter, {
    subscribe: (l: (e: { type: string }) => void) => {
      listeners.add(l);
      const off = base.subscribe(l as never);
      return () => {
        listeners.delete(l);
        off();
      };
    },
    call: (async (op: string, args: Record<string, unknown>) => {
      if (op === "session.signIn") {
        setTimeout(() => {
          waiting = true;
          listeners.forEach((l) => l({ type: "session.changed" }));
        }, 5);
        return { method: "browser", verificationUri: "" };
      }
      const out = await (base.call as (o: string, a: unknown) => Promise<unknown>).call(base, op, args);
      // No browser on this box: the page is to be opened on any device.
      return op === "session.get" && waiting
        ? { ...(out as object), hostSignIn: { userCode: "WDJB-MJHT", url: "https://auth.cua.ai/device", opened: false } }
        : out;
    }) as DataAdapter["call"],
  });
  return adapter;
}

describe("a sign-in the host runs", () => {
  it("shows the code and the page to open on any device, with Copy", async () => {
    render(
      <BridgeProvider adapter={nativeHost()} core={unavailableCore("test: no core")}>
        <AccountFooter />
      </BridgeProvider>,
    );
    fireEvent.click(await screen.findByRole("button", { name: "Sign in" }));
    await waitFor(() => expect(document.querySelector("[data-signin=waiting]")?.textContent).toContain("WDJB-MJHT"));
    expect((await screen.findByRole("status")).textContent).toContain("Open the sign-in page to continue.");
    const link = await waitFor(() => {
      const l = document.querySelector<HTMLElement>("[data-signin-link]");
      if (!l) throw new Error("no link");
      return l;
    });
    expect(link.textContent).toContain("https://auth.cua.ai/device");
    expect(screen.getByRole("button", { name: "Copy the sign-in link" })).toBeTruthy();
  });
});

/** A host whose sign-in fails, with the error a real one gives. */
function failingHost(error: Error) {
  const base = createDemoAdapter({ latencyMs: 0, signedIn: false, onboarded: true });
  return Object.assign(Object.create(base) as DataAdapter, {
    call: ((op: string, args: Record<string, unknown>) =>
      op === "session.signIn"
        ? Promise.reject(error)
        : (base.call as (o: string, a: unknown) => Promise<unknown>).call(base, op, args)) as DataAdapter["call"],
  });
}

describe("a sign-in that fails (it never fails silently)", () => {
  it("says why in the foot, with Try again, Details and a toast", async () => {
    render(
      <ToastProvider>
        <BridgeProvider adapter={failingHost(new Error("could not reach the sign-in service: error sending request"))} core={await testCore()}>
          <AccountFooter />
        </BridgeProvider>
      </ToastProvider>,
    );
    fireEvent.click(await screen.findByRole("button", { name: "Sign in" }));
    const alert = await screen.findByRole("alert");
    // The core's words (or, without the wasm core, the plain fallback line).
    expect(alert.textContent).toMatch(/^Sign-in didn't finish\./);
    expect(screen.getByRole("button", { name: "Try again" })).toBeTruthy();
    // The raw error stays behind Details.
    expect(alert.textContent).not.toContain("error sending request");
    fireEvent.click(screen.getByText("Details"));
    expect(alert.textContent).toContain("error sending request");
    // And a toast (its title is exactly the line's start), once.
    const toasts = await screen.findAllByText("Sign-in didn't finish");
    expect(toasts).toHaveLength(1);
  });

  it("shows who is signed in", async () => {
    const base = createDemoAdapter({ latencyMs: 0, signedIn: true, onboarded: true });
    render(
      <BridgeProvider adapter={base} core={await testCore()}>
        <AccountFooter />
      </BridgeProvider>,
    );
    const who = await waitFor(() => {
      const el = document.querySelector<HTMLElement>("[data-signin-identity]");
      if (!el) throw new Error("not signed in yet");
      return el;
    });
    expect(who.textContent).toMatch(/^Signed in as \S+/);
  });
});
