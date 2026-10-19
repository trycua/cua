// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { StartupAction } from "@/bridge";
import { demoStartup } from "@/bridge/adapters/demo/startup";
import { StartupScreen } from "@/components/startup/StartupScreen";

afterEach(cleanup);

const buttons = () => screen.queryAllByRole("button").map((b) => b.textContent);
const setup = (state = demoStartup("needsKeychain")) => {
  const onAct = vi.fn(async (_a: StartupAction) => {});
  const view = render(<StartupScreen state={state} onAct={onAct} />);
  return { onAct, view };
};

describe("StartupScreen", () => {
  it("needsKeychain: the host's words and its buttons, the first one primary", () => {
    const { onAct } = setup();
    expect(screen.getByRole("heading").textContent).toBe("Allow Keychain access");
    expect(screen.getByText(/needs your permission to read it/)).toBeTruthy();
    expect(buttons()).toEqual(["Allow access", "Sign in again"]);
    expect(screen.getByTestId("startup-screen").querySelector("svg.animate-spin")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Allow access" }));
    expect(onAct).toHaveBeenCalledWith("allowAccess");
  });

  it("waitingForKeychain: a spinner and no buttons, until it is slow", () => {
    const { view } = setup(demoStartup("waitingForKeychain"));
    expect(screen.getByRole("heading").textContent).toBe("Waiting for Keychain access…");
    expect(screen.getByTestId("startup-screen").querySelector("svg.animate-spin")).not.toBeNull();
    expect(buttons()).toEqual([]);
    view.rerender(<StartupScreen state={demoStartup("waitingForKeychain", true)} onAct={async () => {}} />);
    expect(screen.getByRole("heading").textContent).toBe("Still waiting for Keychain access");
    expect(buttons()).toEqual(["Try again", "Sign in again"]);
  });

  it("keychainDenied: Try again goes straight to the host", () => {
    const { onAct } = setup(demoStartup("keychainDenied"));
    expect(screen.getByRole("heading").textContent).toBe("Keychain access was not allowed");
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(onAct).toHaveBeenCalledWith("tryAgain");
  });

  it("starting: a spinner, the title, no body and no buttons", () => {
    setup(demoStartup("starting"));
    expect(screen.getByRole("heading").textContent).toBe("Starting Cua…");
    expect(screen.getByTestId("startup-screen").querySelector("svg.animate-spin")).not.toBeNull();
    expect(screen.getByTestId("startup-screen").querySelector("p")).toBeNull();
    expect(buttons()).toEqual([]);
  });

  it("asks before Sign in again, and Cancel goes back", async () => {
    const { onAct } = setup(demoStartup("keychainDenied"));
    fireEvent.click(screen.getByRole("button", { name: "Sign in again" }));
    expect(onAct).not.toHaveBeenCalled();
    expect(screen.getByTestId("startup-confirm").textContent).toContain(
      "Sign in again? This removes the saved sign-in from this Mac's keychain. Nothing else is deleted. You'll sign in again in your browser.",
    );
    expect(buttons()).toEqual(["Cancel", "Sign in again"]);
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByTestId("startup-confirm")).toBeNull();
    expect(buttons()).toEqual(["Try again", "Sign in again"]);

    fireEvent.click(screen.getByRole("button", { name: "Sign in again" }));
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Sign in again" })));
    expect(onAct).toHaveBeenCalledTimes(1);
    expect(onAct).toHaveBeenCalledWith("signInAgain");
  });
});
