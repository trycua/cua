// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Hardware video decode for the page's WebCodecs (docs/video.md). Chromium
// decodes H.264 on the GPU by itself on macOS (VideoToolbox) and Windows
// (Media Foundation / D3D11); on Linux it ships VA-API decode off, so it is
// turned on here (the GL path, with zero copy where the driver allows it).
// The GPU blocklist still applies: a driver Chromium knows to be broken
// decodes in software, as does any machine without a decoder (the page then
// falls back to software by itself). `CUA_SPACES_VIDEO_DECODE=software`
// turns hardware decode off, to compare. No Electron here.

/** The Linux features that turn VA-API decode on (Chromium's current names). */
export const LINUX_DECODE_FEATURES = ["AcceleratedVideoDecodeLinuxGL", "AcceleratedVideoDecodeLinuxZeroCopyGL"];

/** The command-line switches for video decode on `platform`, merged with any `--enable-features` already given. */
export function videoDecodeSwitches(platform: NodeJS.Platform, env: NodeJS.ProcessEnv, enabledFeatures: string): [string, string?][] {
  if (env.CUA_SPACES_VIDEO_DECODE === "software") return [["disable-accelerated-video-decode"]];
  if (platform !== "linux") return [];
  const features = enabledFeatures.split(",").filter(Boolean);
  for (const f of LINUX_DECODE_FEATURES) if (!features.includes(f)) features.push(f);
  return [["enable-features", features.join(",")]];
}

/** Applies them to Electron's command line (before the app is ready). */
export function applyVideoDecodeSwitches(commandLine: { getSwitchValue(name: string): string; appendSwitch(name: string, value?: string): void }, platform: NodeJS.Platform, env: NodeJS.ProcessEnv): void {
  for (const [name, value] of videoDecodeSwitches(platform, env, commandLine.getSwitchValue("enable-features"))) commandLine.appendSwitch(name, value);
}
