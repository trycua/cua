// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The terminal commands that do what a New Space choice does, for people who
 * would rather type them. Every command and flag here is the `cua` CLI's own
 * (`cua spaces create --help`, `cua sb view --help`); nothing is shown that
 * the CLI does not take.
 *
 * The program's name is the host's (`SettingsSnapshot.cli`): `cua` for the
 * released app and the CLI name of the app's channel for another build
 * (`CuaChannel::cli_name` in cua-home), so a Local build's hints run Local's
 * CLI. The page never names a channel's CLI itself.
 */

import type { CliInfo, CreateSpaceArgs } from "@/bridge";

/** The CLI to show hints for: none when the host did not say, or the CLI is not installed. */
export function shownCli(cli: CliInfo | null | undefined): string | null {
  const name = cli?.name.trim();
  return cli?.installed && name ? name : null;
}

const PLAIN = /^[A-Za-z0-9_@%+=:,./-]+$/;

/** One shell word: as is when it needs no quoting, else in single quotes. */
export function shellWord(word: string): string {
  if (word && PLAIN.test(word)) return word;
  return `'${word.replaceAll("'", `'\\''`)}'`;
}

/** The empty state's create for one system: `cua spaces create linux --on local` (the CLI's image alias). */
export function createAliasCommand(cli: string, os: "linux" | "macos"): string {
  return `${cli} spaces create ${os} --on local`;
}

/**
 * `cua spaces create` with the image, place, kind, runtime, name, resources
 * and GPU New Space chose (`wizard.createArgs`). Null when the place is one
 * the CLI no longer creates on (`cloud`: Cua Cloud has ended) or is not
 * named yet.
 */
export function createCommand(cli: string, args: CreateSpaceArgs): string | null {
  const on = args.on.trim();
  if (!on || on === "cloud" || on === "host:") return null;
  const words = [cli, "spaces", "create", args.image, "--on", on, "--kind", args.kind];
  if (args.runtime && args.runtime !== "auto") words.push("--runtime", args.runtime);
  if (args.name?.trim()) words.push("--name", args.name.trim());
  if (args.cpus) words.push("--cpus", String(args.cpus));
  if (args.memoryMb) words.push("--memory-mb", String(args.memoryMb));
  if (args.diskGb) words.push("--disk-gb", String(args.diskGb));
  if (args.gpu) words.push("--gpu", args.gpu);
  return words.map((w, i) => (i < 3 ? w : shellWord(w))).join(" ");
}

/**
 * `cua sb view NAME`: opens a Space's desktop, as "Open when it's ready"
 * does. Only for a Space on this computer with a name (another computer's
 * Space is not a sandbox here).
 */
export function openCommand(cli: string, args: CreateSpaceArgs): string | null {
  const name = args.name?.trim();
  if (args.on !== "local" || !name) return null;
  return `${cli} sb view ${shellWord(name)}`;
}
