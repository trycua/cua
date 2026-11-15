// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The machine the demo host pretends to be. The browser demo (and the
 * parity flows) are a MacBook Pro on Apple silicon. The Electron shell on
 * Windows or Linux answers from the same demo host with its own system and
 * architecture, so the sample data doesn't talk about Finder, /Volumes or
 * "This Mac" there.
 */

import type { DriveMountInput } from "../../contracts/volume";
import { THIS_MACHINE, type HostOs } from "../../host-os";

export interface DemoPlatform {
  /** The system the demo runs on (a host's, never `unknown`). */
  os: HostOs;
  /** The image catalog's spelling: `arm64` or `amd64`. */
  arch: "arm64" | "amd64";
}

export const MAC_DEMO_PLATFORM: DemoPlatform = { os: "macos", arch: "arm64" };

/** Node's `process.platform` and `process.arch` as a demo platform. */
export function demoPlatformOf(platform: string, arch: string): DemoPlatform {
  const os: HostOs = platform === "win32" ? "windows" : platform === "linux" ? "linux" : "macos";
  return { os, arch: arch === "arm64" || arch === "aarch64" ? "arm64" : "amd64" };
}

/** Everything the demo data says about this machine. */
export interface DemoMachine {
  /** "This Mac", "This PC", "This computer". */
  name: string;
  /** The Machines page's model line. */
  model: string;
  /** `MachineRow.arch` (`aarch64`, `x86_64`). */
  rowArch: string;
  home: string;
  /** Where Cua Volume keeps its files. */
  dataPath: string;
  /** The volume's mount when on, or how it can't be mounted here. */
  mount: (on: boolean) => DriveMountInput;
  /** Local runtimes for New Space. */
  backends: string[];
  /** The disk VMs and images go on, and the container engine's. */
  disk: string;
  containerDisk: string;
  /** Settings, About. */
  osText: string;
}

export function demoMachine(p: DemoPlatform = MAC_DEMO_PLATFORM): DemoMachine {
  const cpu = p.arch === "arm64" ? "arm64" : "x64";
  switch (p.os) {
    case "windows":
      return {
        name: THIS_MACHINE.windows,
        model: "Windows 11 Pro",
        rowArch: p.arch === "arm64" ? "aarch64" : "x86_64",
        home: "C:\\Users\\ada",
        dataPath: "C:\\Users\\ada\\.cua\\volume\\data",
        // No mount on Windows yet: the Storage section shows no mount rows.
        mount: () => ({ enabled: false, state: "unsupported", method: "none", path: null, volume_name: "Cua Volume" }),
        backends: ["docker"],
        disk: "Local Disk (C:)",
        containerDisk: "Docker Desktop",
        osText: `Windows 11 (${cpu})`,
      };
    case "linux":
      return {
        name: THIS_MACHINE.linux,
        model: "Ubuntu 24.04",
        rowArch: p.arch === "arm64" ? "aarch64" : "x86_64",
        home: "/home/ada",
        dataPath: "/home/ada/.cua/volume/data",
        mount: (on) => ({ enabled: on, state: on ? "mounted" : "off", method: "fuse", path: on ? "/home/ada/Cua Volume" : null, volume_name: "Cua Volume" }),
        backends: ["docker", "qemu"],
        disk: "Root (/)",
        containerDisk: "Docker",
        osText: `Ubuntu 24.04 (${cpu})`,
      };
    default:
      return {
        name: THIS_MACHINE.macos,
        model: "MacBook Pro",
        rowArch: "aarch64",
        home: "/Users/ada",
        dataPath: "/Users/ada/.cua/volume/data",
        mount: (on) => ({ enabled: on, state: on ? "mounted" : "off", method: "fskit", path: on ? "/Volumes/Cua Volume" : null, volume_name: "Cua Volume" }),
        backends: ["docker", "lume"],
        disk: "Macintosh HD",
        containerDisk: "Docker Desktop",
        osText: "macOS 26.0 (arm64)",
      };
  }
}
