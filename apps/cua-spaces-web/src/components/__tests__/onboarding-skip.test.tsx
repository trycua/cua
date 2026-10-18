// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { SetUpLater } from "@/components/onboarding-skip";

afterEach(cleanup);

describe("Set up later", () => {
  it("sits left of the window controls overlay, not under it", () => {
    render(<SetUpLater onClick={() => {}} />);
    const button = screen.getByRole("button", { name: "Set up later" });
    expect(button.style.right).toBe("calc(var(--titlebar-right-inset) + 1rem)");
    expect(button.className).not.toMatch(/\bright-\d/);
    expect(button.className).toMatch(/app-no-drag/);
  });

  it("leaves the first run when clicked", () => {
    const onClick = vi.fn();
    render(<SetUpLater onClick={onClick} />);
    fireEvent.click(screen.getByRole("button", { name: "Set up later" }));
    expect(onClick).toHaveBeenCalledTimes(1);
  });
});
