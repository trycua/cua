// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { PromptLine } from "@/components/prompt-line";

afterEach(cleanup);

describe("PromptLine", () => {
  it("is a read-only filled box holding the core's line, with Copy", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, { clipboard: { writeText } });
    const line = "Connect AWS to my Cua Spaces (us-west-2): https://cua.ai/docs/agents/connect-a-cloud";
    render(<PromptLine id="connect-cloud" label="Or ask your agent" text={line} />);
    const box = screen.getByLabelText("Or ask your agent") as HTMLTextAreaElement;
    expect(box.readOnly).toBe(true);
    expect(box.value).toBe(line);
    expect(box.className).not.toMatch(/\bborder\b/);
    fireEvent.click(screen.getByRole("button", { name: /copy/i }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(line));
  });
});
