// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { jobOfSpace, jobRow, oneRowPerJob, sanitizeJobName } from "./job-space";

describe("a job's name", () => {
  it("takes the shape of a pet name", () => {
    expect(sanitizeJobName("NCA Logo!!")).toBe("nca-logo");
    expect(sanitizeJobName("  logo_grower ")).toBe("logo-grower");
    expect(sanitizeJobName("a-very-long-name-that-goes-on-and-on")).toBe("a-very-long-name-that-go");
  });
  it("is never an id", () => {
    for (const id of ["p-train-1a2b3c4d", "cj-0123456789abcdef-1", "p-1a2b3c4d", "", "---", "!!!"]) expect(sanitizeJobName(id), id).toBeNull();
  });
});

describe("a job's row", () => {
  const labels = { "cua.job": "1", "cua.job.name": "logo-grower", "cua.job.gpu": "1" };
  it("shows its name and the GPU icon for a GPU job, the OS icon otherwise", () => {
    expect(jobRow(labels)).toEqual({ name: "logo-grower", gpu: true });
    expect(jobRow({ ...labels, "cua.job.gpu": "" })).toEqual({ name: "logo-grower", gpu: false });
  });
  it("never shows an id, even from a label that holds one", () => {
    expect(jobRow({ ...labels, "cua.job.name": "cj-0123456789abcdef-1" })?.name).toBe("job");
  });
  it("is not a row for any other Space", () => {
    expect(jobRow(undefined)).toBeNull();
    expect(jobRow({ "cua.home": "1" })).toBeNull();
  });
  it("is one row for a job, however many Spaces name its run", () => {
    const a = { id: "local:cj-a-1", jobName: "logo-grower", jobRun: "p-1" };
    const b = { id: "local:cj-a-2", jobName: "logo-grower", jobRun: "p-1" }; // a second attempt of the same run
    const other = { id: "local:browser" };
    expect(oneRowPerJob([a, b, other]).map((s) => s.id)).toEqual(["local:cj-a-1", "local:browser"]);
  });
  it("reads a Space's job fields: GPU icon for a GPU job, the OS icon otherwise, never an id", () => {
    expect(jobOfSpace({ jobName: "logo-grower", jobGpu: true })).toEqual({ name: "logo-grower", gpu: true });
    expect(jobOfSpace({ jobName: "logo-grower" })).toEqual({ name: "logo-grower", gpu: false });
    expect(jobOfSpace({ jobName: "cj-0123456789abcdef-1" })?.name).toBe("job");
    expect(jobOfSpace({})).toBeNull();
  });
});
