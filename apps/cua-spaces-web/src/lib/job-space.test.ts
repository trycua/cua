// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { jobDetail, jobDot, jobOfSpace, jobRow, jobWord, oneRowPerJob, sanitizeJobName, withoutJobSpacesOf } from "./job-space";

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

describe("a job row from the run ledger (logical child, wherever it runs)", () => {
  it("says the state in the list's own words and dots", () => {
    expect([jobWord({ state: "queued" }), jobWord({ state: "starting" }), jobWord({ state: "running" }), jobWord({ state: "failed" })]).toEqual(["Queued", "Starting", "Running", "Failed"]);
    expect([jobDot({ state: "running" }), jobDot({ state: "failed" }), jobDot({ state: "queued" }), jobDot({ state: "starting" })]).toEqual(["running", "failed", "creating", "creating"]);
  });
  it("shows where it runs only as a quiet detail, and a gang's size", () => {
    expect(jobDetail({ host: "mac-m2max", members: 1 })).toBe("on mac-m2max");
    expect(jobDetail({ host: "", members: 1 })).toBe("");
    expect(jobDetail({ host: "3 computers", members: 3 })).toBe("on 3 computers · 3 members");
  });
  it("is one row per job: the job's own Space on this computer is not listed again", () => {
    const local = { id: "local:cj-a-1", jobName: "logo-grower", jobRun: "local/p-1" };
    const other = { id: "local:browser" };
    expect(withoutJobSpacesOf([local, other], [{ run: "p-1" }]).map((s) => s.id)).toEqual(["local:browser"]);
    expect(withoutJobSpacesOf([local, other], [{ run: "p-2" }]).map((s) => s.id)).toEqual(["local:cj-a-1", "local:browser"]);
    expect(withoutJobSpacesOf([local], [])).toEqual([local]);
  });
});
