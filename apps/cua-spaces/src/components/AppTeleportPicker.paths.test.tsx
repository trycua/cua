// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { createFallbackTeleportAppsBridge, FIXTURE_CATALOG } from "../native/teleportApps";
import { AppTeleportPicker } from "./AppTeleportPicker";

describe("AppTeleportPicker paths", () => {
  it("shows chosen files under the home folder as ~/...", async () => {
    const host = createFallbackTeleportAppsBridge().host("space");
    const vscode = FIXTURE_CATALOG.find((e) => e.id === "vscode")!;
    render(
      <AppTeleportPicker
        host={host}
        spaceName="Aurora"
        preselect={{ entry: vscode, files: ["/Users/ada/Projects/demo"] }}
        home="/Users/ada"
        onClose={() => {}}
      />,
    );
    const file = await screen.findByText("~/Projects/demo");
    expect(file).toHaveAttribute("title", "/Users/ada/Projects/demo");
    expect(screen.queryByText("/Users/ada/Projects/demo")).toBeNull();
  });
});
