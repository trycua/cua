// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { rowToSpace } from "../model/spaces";
import { spaceDetail } from "../model/window";
import { FactList } from "./FactList";

const DIGEST = "sha256:4f1c2a9d8e7b6c5a4f3e2d1c0b9a8f7e6d5c4b3a2f1e0d9c8b7a6f5e4d3c2b1a";

const space = rowToSpace(
  {
    id: "local:aurora",
    name: "aurora",
    provider: "local",
    spacesdVersion: "0.4.0",
    features: ["desktop_stream"],
    os: "linux",
    osName: "Ubuntu",
    osPrettyName: "Ubuntu 24.04.3 LTS",
    image: "ghcr.io/trycua/linux:24.04-slim",
    imageDigest: DIGEST,
    reachable: true,
  },
  0,
);

const usage = {
  memoryUsed: 2_254_857_830,
  memoryTotal: 4 * 1024 ** 3,
  memoryLimited: true,
  diskUsed: 150e9,
  diskTotal: 494e9,
  diskLimited: false,
};

const row = (label: string) => screen.getByText(label).closest("div") as HTMLElement;

describe("FactList (the Space detail's facts)", () => {
  it("shows Image, the full System string, Memory, and no Location", () => {
    render(<FactList facts={spaceDetail(space, usage).facts} />);
    const labels = Array.from(document.querySelectorAll("dt")).map((e) => e.textContent);
    expect(labels).toEqual(["Status", "Image", "Identifier", "System", "Kind", "Memory"]);
    expect(within(row("Kind")).getByText("Container")).toBeInTheDocument();
    expect(within(row("Image")).getByText("ghcr.io/trycua/linux:24.04-slim")).toHaveAttribute(
      "title",
      `ghcr.io/trycua/linux:24.04-slim\n${DIGEST}`,
    );
    expect(within(row("System")).getByText("Ubuntu 24.04.3 LTS")).toBeInTheDocument();
    // The OS icon is the sidebar's; the System row is the string alone.
    expect(row("System").querySelector("[data-os-icon]")).toBeNull();
    expect(within(row("Memory")).getByText("2.1 GB / 4 GB")).toBeInTheDocument();
    expect(screen.queryByText("Storage")).toBeNull();
  });

  it("warns on an emulated local Space's Architecture, with the core's tooltip", () => {
    const omarchy = rowToSpace(
      {
        id: "local:omarchy",
        name: "omarchy",
        provider: "local",
        spacesdVersion: "0.4.0",
        features: ["desktop_stream"],
        os: "linux",
        image: "ghcr.io/trycua/omarchy:edge",
        kind: "vm",
        arch: "amd64",
        reachable: true,
      },
      0,
    );
    const { rerender } = render(<FactList facts={spaceDetail(omarchy, null, "aarch64").facts} />);
    expect(within(row("System")).getByText("Omarchy")).toBeInTheDocument();
    expect(within(row("Kind")).getByText("Virtual machine")).toBeInTheDocument();
    const help = "Emulated on this Mac\u2019s ARM processor. Performance may be degraded.";
    expect(within(row("Architecture")).getByText("x64")).toBeInTheDocument();
    expect(within(row("Architecture")).getByRole("img", { name: help })).toHaveAttribute("title", help);
    // Native: no warning.
    rerender(<FactList facts={spaceDetail(omarchy, null, "x86_64").facts} />);
    expect(within(row("Architecture")).queryByRole("img")).toBeNull();
  });

  it("drops Memory and Storage until usage arrives", () => {
    render(<FactList facts={spaceDetail(space).facts} />);
    expect(screen.queryByText("Memory")).toBeNull();
  });

  it("copies the image ref and the identifier", async () => {
    const write = vi.fn(async () => {});
    render(<FactList facts={spaceDetail(space, usage).facts} copyText={write} />);
    await act(async () => {
      fireEvent.click(within(row("Image")).getByRole("button", { name: "Copy" }));
    });
    expect(write).toHaveBeenLastCalledWith("ghcr.io/trycua/linux:24.04-slim");
    await act(async () => {
      fireEvent.click(within(row("Identifier")).getByRole("button", { name: "Copy" }));
    });
    expect(write).toHaveBeenLastCalledWith("local:aurora");
  });
});
