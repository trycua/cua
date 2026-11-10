// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { jobOfSpace } from "@/lib/job-space";
import { GpuIcon, SpaceIcon } from "./gpu-icon";

describe("a child Space's icon", () => {
  it("draws the dual-fan card for a GPU job", () => {
    const { container } = render(<SpaceIcon os="linux" gpu={jobOfSpace({ jobName: "logo-grower", jobGpu: true })?.gpu} />);
    const svg = container.querySelector("svg[data-gpu-icon]");
    expect(svg).not.toBeNull();
    expect(svg?.getAttribute("fill")).toBe("none");
    expect(svg?.getAttribute("stroke-width")).toBe("1.5");
    expect(svg?.querySelectorAll("circle")).toHaveLength(2);
    expect(svg?.getAttribute("aria-label")).toBe("GPU");
  });

  it("keeps the OS logo for a job without a GPU and for any other Space", () => {
    const job = render(<SpaceIcon os="linux" gpu={jobOfSpace({ jobName: "logo-grower" })?.gpu} />);
    expect(job.container.querySelector("svg[data-gpu-icon]")).toBeNull();
    expect(job.container.querySelector("svg")?.getAttribute("aria-label")).toBe("Linux");
    const plain = render(<SpaceIcon os="macos" />);
    expect(plain.container.querySelector("svg")?.getAttribute("aria-label")).toBe("macOS");
  });

  it("takes the caller's size like the OS icon", () => {
    const { container } = render(<GpuIcon className="size-3.5 shrink-0 text-muted-foreground" />);
    expect(container.querySelector("svg")?.getAttribute("class")).toContain("size-3.5");
  });
});
