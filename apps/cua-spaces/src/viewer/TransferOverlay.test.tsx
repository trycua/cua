// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { TransferOverlayState } from "../model/transfer";
import { TransferOverlayView } from "./SpaceViewer";

describe("TransferOverlayView", () => {
  it("shows a determinate bar and a '{x} MB / {y} MB' label once bytes arrive", () => {
    const state: TransferOverlayState = {
      phase: "active",
      appName: "Google Chrome",
      sentBytes: 12.3 * 1024 * 1024,
      totalBytes: 131.5 * 1024 * 1024,
    };
    render(<TransferOverlayView state={state} onRetry={() => {}} onCancel={() => {}} />);

    const bar = screen.getByTestId("transfer-bar");
    expect(bar).toHaveAttribute("data-determinate", "true");
    expect(bar).toHaveAttribute("role", "progressbar");
    // 12.3 / 131.5 ≈ 9% -> aria-valuenow rounded.
    expect(bar).toHaveAttribute("aria-valuenow", "9");

    // Fill width tracks sent/total.
    expect(screen.getByTestId("transfer-fill")).toHaveStyle({ width: "9.4%" });

    // Human-readable size label.
    expect(screen.getByTestId("transfer-size")).toHaveTextContent("12.3 MB / 131.5 MB");
  });

  it("falls back to the indeterminate sweep before any bytes are known", () => {
    const state: TransferOverlayState = { phase: "active", appName: "Google Chrome" };
    render(<TransferOverlayView state={state} onRetry={() => {}} onCancel={() => {}} />);

    const bar = screen.getByTestId("transfer-bar");
    expect(bar).not.toHaveAttribute("data-determinate");
    expect(bar).toHaveAttribute("aria-hidden", "true");
    expect(screen.queryByTestId("transfer-fill")).toBeNull();
    expect(screen.queryByTestId("transfer-size")).toBeNull();
    expect(screen.getByText("Transferring the session…")).toBeInTheDocument();
  });

  it("shows the failure message with Retry and Cancel in the error phase", () => {
    const onRetry = vi.fn();
    const onCancel = vi.fn();
    const state: TransferOverlayState = {
      phase: "error",
      appName: "Google Chrome",
      message: "The receiver rejected the import.",
    };
    render(<TransferOverlayView state={state} onRetry={onRetry} onCancel={onCancel} />);

    expect(screen.getByText("The receiver rejected the import.")).toBeInTheDocument();
    expect(screen.queryByTestId("transfer-bar")).toBeNull();

    // Retry re-runs the push (in Rust); Cancel dismisses back to the live stream.
    screen.getByRole("button", { name: "Retry" }).click();
    expect(onRetry).toHaveBeenCalledOnce();
    expect(onCancel).not.toHaveBeenCalled();
    screen.getByRole("button", { name: "Cancel" }).click();
    expect(onCancel).toHaveBeenCalledOnce();
  });
});
