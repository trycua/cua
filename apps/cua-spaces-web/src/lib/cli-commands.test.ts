// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import type { CreateSpaceArgs } from "@/bridge";

import { createAliasCommand, createCommand, openCommand, shellWord, shownCli } from "./cli-commands";

// The names a host reports: the released app's, and a Local build's
// (`CuaChannel::cli_name`). The builder takes either as given.
const STABLE = "cua";
const LOCAL = "cua-local";

const linux: CreateSpaceArgs = { image: "ghcr.io/trycua/linux:24.04", on: "local", kind: "container", runtime: "auto", spacesd: true };
const macos: CreateSpaceArgs = { image: "ghcr.io/trycua/macos:26", on: "local", kind: "vm", runtime: "lume", spacesd: true };

describe("shownCli", () => {
  it("names the host's CLI only when it is installed", () => {
    expect(shownCli({ name: STABLE, installed: true })).toBe("cua");
    expect(shownCli({ name: LOCAL, installed: true })).toBe("cua-local");
    expect(shownCli({ name: STABLE, installed: false })).toBeNull();
    expect(shownCli({ name: " ", installed: true })).toBeNull();
    expect(shownCli(null)).toBeNull();
    expect(shownCli(undefined)).toBeNull();
  });
});

describe("the empty state's creates", () => {
  it("use the CLI's image aliases, under the channel's name", () => {
    expect(createAliasCommand(STABLE, "linux")).toBe("cua spaces create linux --on local");
    expect(createAliasCommand(STABLE, "macos")).toBe("cua spaces create macos --on local");
    expect(createAliasCommand(LOCAL, "linux")).toBe("cua-local spaces create linux --on local");
    expect(createAliasCommand(LOCAL, "macos")).toBe("cua-local spaces create macos --on local");
  });
});

describe("New Space's create", () => {
  it("names the image, the place and the kind; an automatic runtime is the CLI's default", () => {
    expect(createCommand(STABLE, linux)).toBe("cua spaces create ghcr.io/trycua/linux:24.04 --on local --kind container");
    expect(createCommand(LOCAL, linux)).toBe("cua-local spaces create ghcr.io/trycua/linux:24.04 --on local --kind container");
  });

  it("passes a chosen runtime, name, resources and GPU", () => {
    expect(createCommand(STABLE, { ...macos, name: "studio", cpus: 4, memoryMb: 8192, diskGb: 80, gpu: "paravirtual" })).toBe(
      "cua spaces create ghcr.io/trycua/macos:26 --on local --kind vm --runtime lume --name studio --cpus 4 --memory-mb 8192 --disk-gb 80 --gpu paravirtual",
    );
    expect(createCommand(LOCAL, { ...linux, kind: "vm", runtime: "qemu" })).toBe("cua-local spaces create ghcr.io/trycua/linux:24.04 --on local --kind vm --runtime qemu");
  });

  it("runs on another of your machines, or in your cloud", () => {
    expect(createCommand(STABLE, { ...linux, on: "host:0123abcd4567ef89" })).toBe("cua spaces create ghcr.io/trycua/linux:24.04 --on host:0123abcd4567ef89 --kind container");
    expect(createCommand(STABLE, { ...linux, on: "aws" })).toBe("cua spaces create ghcr.io/trycua/linux:24.04 --on aws --kind container");
    expect(createCommand(STABLE, { ...linux, on: "auto" })).toBe("cua spaces create ghcr.io/trycua/linux:24.04 --on auto --kind container");
  });

  it("quotes what the shell would split", () => {
    expect(createCommand(STABLE, { ...linux, name: "Dan's box" })).toBe(`cua spaces create ghcr.io/trycua/linux:24.04 --on local --kind container --name 'Dan'\\''s box'`);
    expect(shellWord("plain-name_1")).toBe("plain-name_1");
    expect(shellWord("two words")).toBe("'two words'");
    expect(shellWord("")).toBe("''");
  });

  it("shows nothing where the CLI can't create (Cua Cloud has ended, no machine picked)", () => {
    expect(createCommand(STABLE, { ...linux, on: "cloud" })).toBeNull();
    expect(createCommand(STABLE, { ...linux, on: "host:" })).toBeNull();
    expect(createCommand(STABLE, { ...linux, on: "" })).toBeNull();
  });
});

describe("opening the Space", () => {
  it("is `sb view NAME` for a named Space on this machine", () => {
    expect(openCommand(STABLE, { ...linux, name: "dev" })).toBe("cua sb view dev");
    expect(openCommand(LOCAL, { ...linux, name: "my space" })).toBe("cua-local sb view 'my space'");
  });

  it("is not offered without a name, or for another machine's Space", () => {
    expect(openCommand(STABLE, linux)).toBeNull();
    expect(openCommand(STABLE, { ...linux, name: "dev", on: "host:abc" })).toBeNull();
  });
});
