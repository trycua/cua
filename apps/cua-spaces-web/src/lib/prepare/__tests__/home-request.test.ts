// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// CUA-3193: the new setup ended on "Preparing stopped: config.image: string". It made the first
// Space before this machine's offers were read, so the create went out without an image and the
// host refused it with its argument check. The create now comes from the wizard's Linux offer.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import { testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import type { WizardEnv } from "@/bridge/contracts/new-space";
import { firstSpaceOffers, type FirstSpaceOffer } from "@/bridge/new-space";

import { homeCreateRequest, NO_LINUX_OFFER } from "../home-request";

/** The first-run parity fixture's Mac (arm64, 200 GB free, Lume and the built-in Linux runtime). */
const MAC = JSON.parse(readFileSync(join(import.meta.dirname, "../../../../../../libs/cua/crates/cua-spaces-app-core/parity/first-run.json"), "utf8"))
  .machineBase as WizardEnv;

describe("the new setup's first Space create", () => {
  it("never goes out without an image", () => {
    expect(() => homeCreateRequest([], "home-claude-code")).toThrow(NO_LINUX_OFFER);
    const blocked = {
      id: "linux",
      os: "linux",
      image: "ghcr.io/trycua/linux:24.04-slim",
      name: "Ubuntu",
      size: null,
      time: "",
      blocked: "Not enough free disk space.",
      args: { image: "ghcr.io/trycua/linux:24.04-slim", on: "local", kind: "container", runtime: "auto", spacesd: true },
    } as FirstSpaceOffer;
    expect(() => homeCreateRequest([blocked], "home-claude-code")).toThrow("Not enough free disk space.");
    // The plain words, never the host's argument check.
    expect(NO_LINUX_OFFER).not.toMatch(/config|image:|string/);
  });

  it("is labelled hidden and off the relay: out of the top bar and off the phone's list until a person writes into it", () => {
    const offer = {
      id: "linux",
      os: "linux",
      image: "ghcr.io/trycua/linux:24.04-slim",
      name: "Ubuntu",
      size: null,
      time: "",
      blocked: null,
      args: { image: "ghcr.io/trycua/linux:24.04-slim", on: "local", kind: "container", runtime: "auto", spacesd: true },
    } as FirstSpaceOffer;
    expect(homeCreateRequest([offer], "home-claude-code")).toMatchObject({ name: "home-claude-code", labels: { "cua.hidden": "home", "cua.relay": "off" } });
  });

  it.skipIf(!wasmBuilt)("is the wizard's Linux offer for this machine, under the home's name", async () => {
    const core = await testCore();
    const offers = firstSpaceOffers(core, MAC, "macos");
    const linux = offers.find((o) => o.id === "linux")!;
    expect(linux.blocked).toBeNull();
    const req = homeCreateRequest(offers, "home-claude-code");
    expect(typeof req.image).toBe("string");
    expect(req.image).toBe(linux.args.image);
    expect(req.image).toMatch(/^ghcr\.io\/trycua\/linux:/);
    expect(req).toMatchObject({ name: "home-claude-code", os: "linux", kind: linux.args.kind, runtime: linux.args.runtime, on: "local" });
  });
});
