// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The sandbox images the New Space wizard offers: ONE file,
 * `libs/images/sandbox-images.json` (the docs render it too), read by the
 * app core (`cua-spaces-app-core::wizard`). Only `published` entries are
 * offered. Placement, kind and runtime rules are the core's.
 */
import { core } from "../core";
import type { Location, Runtime, SpaceKind } from "./types";

export type ImageOs = "linux" | "windows" | "macos";
/** The catalog's image tier: `slim` (what CI runs), `full` (the default,
 * with dev tooling) or `xcode` (macOS: full plus Xcode). */
export type ImageTier = "slim" | "full" | "xcode";
/** The image list's local engine: `container` (gVisor when installed, else
 * runc), `qemu` or `lume`. */
export type LocalEngine = "container" | "qemu" | "lume";
/** The image list's cloud engine. */
export type CloudEngine = "gvisor" | "kubevirt";

export interface SandboxImage {
  ref: string;
  group: string;
  os: ImageOs;
  name: string;
  variant: SpaceKind;
  summary: string;
  spacesd: boolean;
  local: LocalEngine | null;
  cloud: CloudEngine | null;
  /** `slim`, `full` (the default) or `xcode`: canonical images only. */
  tier?: ImageTier;
  published: boolean;
}

export interface ImageGroup {
  id: string;
  label: string;
  images: SandboxImage[];
}

/** Where the wizard creates: a location, the user's own cloud, or one of
 * the user's machines that provides Spaces. */
export type Placement = Location | 'yours' | 'host';

/** Every image the pickers show, in file order. */
export function pickerImages(): SandboxImage[] {
  return core("wizard.pickerImages");
}

/** The picker images grouped as the file groups them. */
export function pickerGroups(): ImageGroup[] {
  return core("wizard.pickerGroups");
}

export function findImage(ref: string): SandboxImage | undefined {
  return core<SandboxImage | null>("wizard.findImage", { ref }) ?? undefined;
}

export function canPlace(image: SandboxImage, placement: Placement): boolean {
  return core("wizard.canPlace", { image, on: placement });
}

/** The engines `image` can run on in `on`, the default first. */
export function runtimeOptions(image: SandboxImage, on: Location): Runtime[] {
  return core("wizard.runtimeOptions", { image, on });
}

/** The kinds `image` comes in that can run in `on`. */
export function kindOptions(image: SandboxImage, on: Location): { kind: SpaceKind; image: SandboxImage }[] {
  return core("wizard.kindOptions", { image, on });
}

/** Can this Mac run `image` locally, given the ready backends? */
export function localRuntimeReady(image: SandboxImage, backends: readonly string[]): boolean {
  return core("wizard.localRuntimeReady", { image, backends });
}
