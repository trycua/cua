// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ExamplePrompts } from "@/components/onboarding-prompts";

afterEach(cleanup);

const PROMPTS = ["Open a Linux Space and run my test suite in it", "Share this Space with a teammate as a viewer"];

describe("Done's example prompts", () => {
  it("lists each prompt under 'Try asking your agent:' and copies one on click", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    render(<ExamplePrompts prompts={PROMPTS} />);
    expect(screen.getByText("Try asking your agent:")).toBeTruthy();
    expect(screen.getAllByRole("button")).toHaveLength(2);
    await act(async () => {
      fireEvent.click(screen.getByText(`“${PROMPTS[1]}”`));
    });
    expect(writeText).toHaveBeenCalledWith(PROMPTS[1]);
    expect(screen.getByText("Copied")).toBeTruthy();
  });

  it("shows nothing without prompts", () => {
    const { container } = render(<ExamplePrompts prompts={[]} />);
    expect(container.textContent).toBe("");
  });
});
