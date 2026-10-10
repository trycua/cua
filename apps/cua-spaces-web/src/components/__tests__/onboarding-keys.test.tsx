// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { pressPrimaryOnReturn } from "@/components/onboarding-keys";

afterEach(cleanup);

function page(primaryDisabled = false) {
  const primary = vi.fn();
  const other = vi.fn();
  render(
    <div data-testid="page" tabIndex={-1} onKeyDown={pressPrimaryOnReturn}>
      <h1 tabIndex={-1} data-testid="heading">Sign in</h1>
      <button type="button" onClick={other}>Skip</button>
      <input data-testid="field" />
      <button type="button" data-onboarding-primary="" disabled={primaryDisabled} onClick={primary}>
        Continue
      </button>
    </div>,
  );
  return { primary, other };
}

describe("Return on a first-run page", () => {
  it("presses the primary button from the page (the heading has focus after a step change)", () => {
    const { primary, other } = page();
    fireEvent.keyDown(screen.getByTestId("heading"), { key: "Enter" });
    expect(primary).toHaveBeenCalledTimes(1);
    expect(other).not.toHaveBeenCalled();
  });

  it("leaves Return to a focused control, and to a disabled primary", () => {
    const { primary } = page();
    fireEvent.keyDown(screen.getByText("Skip"), { key: "Enter" });
    fireEvent.keyDown(screen.getByTestId("field"), { key: "Enter" });
    fireEvent.keyDown(screen.getByTestId("heading"), { key: "Enter", metaKey: true });
    expect(primary).not.toHaveBeenCalled();
    cleanup();
    const disabled = page(true);
    fireEvent.keyDown(screen.getByTestId("heading"), { key: "Enter" });
    expect(disabled.primary).not.toHaveBeenCalled();
  });
});
