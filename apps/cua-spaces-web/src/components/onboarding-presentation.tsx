// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { OnboardingHook, OnboardingView } from "@/bridge";
import { cn } from "@/lib/utils";

/** The Menu bar page's choices, side by side, as the SwiftUI page draws them:
 * a miniature of the top of the screen, and the card's title. */
export function PresentationCards({ flow, view }: { flow: OnboardingHook; view: OnboardingView }) {
  return (
    <div role="radiogroup" aria-label={view.title} className="grid grid-cols-2 gap-3">
      {(view.presentations ?? []).map((card) => (
        <button
          key={card.id}
          type="button"
          role="radio"
          aria-checked={card.selected}
          data-testid={card.id}
          onClick={() => flow.send({ type: "presentation-picked", menuBar: card.menuBar })}
          className={cn(
            "flex flex-col items-center gap-2 rounded-xl border bg-card p-2 shadow-xs outline-none transition-[border-color,box-shadow]",
            "hover:border-foreground/20 focus-visible:border-brand focus-visible:ring-3 focus-visible:ring-brand/25",
            card.selected && "border-foreground/40",
          )}
        >
          <ScreenTop notch={!card.menuBar} label={card.imageLabel} />
          <span className="text-[12.5px] font-medium">{card.title}</span>
        </button>
      ))}
    </div>
  );
}

/** The top of a Mac's screen: the menu bar with the app's icon, and the
 * notch where the Spaces tiles open (when it shows there). */
function ScreenTop({ notch, label }: { notch: boolean; label: string }) {
  return (
    <div role="img" aria-label={label} className="relative h-20 w-full overflow-hidden rounded-md bg-gradient-to-b from-sky-200 to-indigo-300 dark:from-slate-700 dark:to-indigo-950">
      <div className="absolute inset-x-0 top-0 flex h-3 items-center justify-end gap-1 bg-white/50 px-1.5 dark:bg-black/40">
        <span className="size-1.5 rounded-full bg-foreground/70" />
        <span className="h-1 w-3 rounded-full bg-foreground/30" />
      </div>
      {notch ? (
        <div className="absolute top-0 left-1/2 h-3 w-12 -translate-x-1/2 rounded-b-md bg-black">
          <div className="absolute top-3 left-1/2 flex -translate-x-1/2 gap-1 rounded-b-md bg-black px-1.5 pt-0.5 pb-1.5">
            <span className="h-3 w-4 rounded-[2px] bg-white/70" />
            <span className="h-3 w-4 rounded-[2px] bg-white/40" />
            <span className="h-3 w-4 rounded-[2px] bg-white/40" />
          </div>
        </div>
      ) : null}
    </div>
  );
}
