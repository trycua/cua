// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { rowToSpace } from "../model/spaces";
import { spaceDetail } from "../model/window";
import { CopyButton } from "./CopyButton";

const ID = "direct:127.0.0.1:34752";

/** The Identifier fact's copy button, as the core describes it. */
function identifierCopy() {
  const space = rowToSpace(
    {
      id: ID,
      name: "studio",
      provider: "direct",
      spacesdVersion: "0.4.0",
      features: [],
      os: "linux",
      reachable: true,
    },
    0,
  );
  const facts = spaceDetail(space).facts;
  expect(facts.filter((f) => f.copy).map((f) => f.label)).toEqual(["Identifier"]);
  return facts.find((f) => f.copy)!.copy!;
}

afterEach(() => vi.useRealTimers());

describe("CopyButton (the Identifier row)", () => {
  it("copies the full id, shows Copied, then goes back", async () => {
    vi.useFakeTimers();
    const copy = identifierCopy();
    expect(copy).toMatchObject({ text: ID, symbol: "doc.on.doc", help: "Copy", doneSymbol: "checkmark", doneHelp: "Copied" });
    const write = vi.fn(async () => {});
    render(<CopyButton copy={copy} write={write} />);
    const button = screen.getByRole("button", { name: "Copy" });
    expect(button).toHaveAttribute("title", "Copy");
    await act(async () => {
      fireEvent.click(button);
    });
    expect(write).toHaveBeenCalledWith(ID);
    expect(screen.getByRole("button", { name: "Copied" })).toHaveAttribute("data-copied", "true");
    await act(async () => {
      vi.advanceTimersByTime(copy.confirmMs);
    });
    expect(screen.getByRole("button", { name: "Copy" })).not.toHaveAttribute("data-copied");
  });

  it("stays on Copy when the clipboard refuses", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const write = vi.fn(async () => {
      throw new Error("denied");
    });
    render(<CopyButton copy={identifierCopy()} write={write} />);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Copy" }));
    });
    expect(screen.getByRole("button", { name: "Copy" })).toBeInTheDocument();
    expect(warn).toHaveBeenCalled();
    warn.mockRestore();
  });
});
