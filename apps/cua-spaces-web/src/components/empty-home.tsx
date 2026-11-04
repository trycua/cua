// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The Spaces page with no Spaces yet, quiet and centered: one heading, one
 * line, a soft button per system this machine runs ("Linux Space", and
 * "macOS Space" on Apple silicon Macs) with one muted figure each, and the
 * same creates as terminal commands behind "Prefer the terminal?" when the
 * app's CLI is installed. A row opens
 * New Space with that system chosen, on this machine (the wizard says when
 * it can't run here). Shown once, it counts `first_run_guide_shown`.
 */

import { useNewSpaceWizard, type EmptyHome as EmptyHomeView, type EmptyTile } from "@/bridge";
import { useEffect, useState } from "react";

import { CliHintLine, useCliName } from "@/components/cli-hint";
import { useFirstRunStep } from "@/components/first-run-guide";
import { OsIcon } from "@/components/os-icon";
import { createAliasCommand } from "@/lib/cli-commands";

/** The first of the core's figures ("About 4 minutes" of "About 4 minutes · 1.0–2.0 GB of disk"): one muted detail, no dot-chain. */
export const firstFigure = (detail: string) => detail.split(" \u00b7 ")[0]!.trim();

/** `unread`: the host could not read the Spaces list (`listNotice`): the
 * title says so and no system is offered, as the SwiftUI app's does. */
export function EmptyHome({ home, unread = false }: { home: EmptyHomeView; unread?: boolean }) {
  const wizard = useNewSpaceWizard();
  const start = (tile: EmptyTile) => wizard.show(null, tile.os);
  const step = useFirstRunStep();
  const [terminal, setTerminal] = useState(false);
  // The same creates in a terminal, when the app's CLI is on this computer.
  const cli = useCliName();
  const commands = cli ? home.tiles.map((tile) => createAliasCommand(cli, tile.os)) : [];
  useEffect(() => {
    if (!unread) step("guide_shown");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [unread]);
  return (
    <section data-empty-home="" aria-labelledby="empty-home-title" className="flex flex-col items-center px-8 pt-16 pb-10 text-center">
      <h2 id="empty-home-title" className="text-[17px] font-semibold text-foreground">
        {unread ? "Spaces could not be loaded" : "Create your first Space"}
      </h2>
      {unread ? null : (
        <>
          <p className="mt-1.5 text-[13px] text-muted-foreground">A separate computer on This Mac, for you and your agents.</p>
          <div className="mt-6 flex flex-wrap justify-center gap-3">
            {home.tiles.map((tile) => (
              <SystemButton key={tile.os} tile={tile} onStart={start} />
            ))}
          </div>
          {commands.length ? (
            <div className="mt-5 flex flex-col items-center">
              <button
                type="button"
                data-empty-terminal=""
                aria-expanded={terminal}
                onClick={() => setTerminal(!terminal)}
                className="text-xs text-muted-foreground outline-none hover:text-foreground focus-visible:underline"
              >
                Prefer the terminal?
              </button>
              {terminal ? <CliHintLine commands={commands} label={null} className="mt-2" /> : null}
            </div>
          ) : null}
        </>
      )}
    </section>
  );
}

/** "Linux Space" with its mark, and one muted figure under it: a soft tonal button, no outline. */
function SystemButton({ tile, onStart }: { tile: EmptyTile; onStart: (t: EmptyTile) => void }) {
  const detail = firstFigure(tile.detail);
  const label = `${tile.name} Space`;
  return (
    <button
      type="button"
      data-empty-tile={tile.os}
      aria-label={`${label}, ${detail}`}
      onClick={() => onStart(tile)}
      className="flex w-52 cursor-default flex-col items-center gap-1 rounded-lg bg-foreground/[0.05] px-4 py-4 outline-none transition-colors hover:bg-foreground/[0.08] focus-visible:ring-2 focus-visible:ring-ring/60 active:bg-foreground/[0.1]"
    >
      <span className="flex items-center gap-2 text-[13px] font-medium">
        <OsIcon os={tile.os} className="size-4 shrink-0" />
        {label}
      </span>
      <span data-empty-tile-detail="" className="text-xs text-muted-foreground">
        {detail}
      </span>
    </button>
  );
}
