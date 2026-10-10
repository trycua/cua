// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Radio } from "@base-ui/react/radio";
import { RadioGroup } from "@base-ui/react/radio-group";
import { LoaderCircleIcon, SearchIcon } from "lucide-react";
import type { ReactNode } from "react";

import { useTeleport, type PickerState, type TeleportMove, type TeleportSession, type TeleportStore } from "@/bridge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Segmented } from "@/components/ui/segmented";

import { TeleportGrid } from "./grid";
import { TeleportReview } from "./review";

/** What each move is called (the SwiftUI app's `AppTeleportMove.label`, the core's `Move::label`). */
export const MOVE_LABEL: Record<TeleportMove, string> = {
  app_only: "Just the app",
  app_with_files: "The app with files or folders",
  app_with_state: "The app with its signed-in state",
};

/**
 * "Teleport an app": pick, choose what moves, review every install, item
 * and secret, then run. The steps, the grid and the review gate are the
 * app core's (`useTeleport`); this only draws them, as the SwiftUI app's
 * `TeleportPickerSheet` does. A window dragged to the notch stays native.
 */
export default function TeleportDialog() {
  const { session, images, teleport } = useTeleport();
  if (!session || !teleport) return null;
  return (
    <Dialog open onOpenChange={(open) => !open && teleport.close()}>
      <DialogPopup
        data-teleport
        data-step={session.state.step}
        className="top-[8vh] flex h-[min(600px,84vh)] w-[min(700px,calc(100vw-2rem))] flex-col"
      >
        <DialogTitle className="shrink-0 px-5 pt-4 pb-3 text-[15px] font-semibold" data-teleport-title>
          {title(session)}
        </DialogTitle>
        <div className="min-h-0 flex-1 overflow-y-auto">
          <Body session={session} teleport={teleport} images={images} />
        </div>
        <Footer session={session} teleport={teleport} />
      </DialogPopup>
    </Dialog>
  );
}

function title(s: TeleportSession): string {
  if (s.frame.review) return s.frame.review.title;
  if (s.state.entry) return s.state.entry.name;
  return `Teleport an app to ${s.spaceName}`;
}

function Body({ session: s, teleport, images }: { session: TeleportSession; teleport: TeleportStore; images: ReadonlyMap<string, string | null> }) {
  switch (s.state.step) {
    case "loading":
      return <Busy>Loading apps…</Busy>;
    case "planning":
      return <Busy>Planning…</Busy>;
    case "pick":
      return (
        <div className="flex flex-col gap-3 px-5 pb-4">
          <Segmented
            aria-label="Show"
            className="self-start"
            value={s.tab}
            options={s.tabs.map((t) => ({ value: t.tab, label: <span data-teleport-tab={t.tab}>{t.label}</span> }))}
            onValueChange={(tab) => teleport.setTab(tab)}
          />
          <div className="relative">
            <SearchIcon className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground" />
            <Input
              aria-label="Search"
              placeholder="Search"
              className="pl-8"
              value={s.query}
              onChange={(e) => teleport.setQuery(e.target.value)}
            />
          </div>
          {s.tab === "space" && s.remoteError ? <p className="text-xs text-muted-foreground">{s.remoteError}</p> : null}
          <TeleportGrid session={s} teleport={teleport} images={images} />
        </div>
      );
    case "options":
      return <Options state={s.state} session={s} teleport={teleport} />;
    case "consent":
      return s.frame.review ? <TeleportReview review={s.frame.review} readingSites={s.readingSites} vault={s.vault} teleport={teleport} /> : null;
    case "running":
      return <Running session={s} />;
    case "done":
      return (
        <p className="px-5 py-4 text-[13px]" data-teleport-done>
          Done
        </p>
      );
    case "error":
      return (
        <p className="px-5 py-4 text-[13px] text-muted-foreground" data-teleport-error>
          {s.state.installPrompt?.message ?? s.state.error}
        </p>
      );
  }
}

function Busy({ children }: { children: ReactNode }) {
  return (
    <div className="flex h-full items-center justify-center gap-2 text-[13px] text-muted-foreground" data-teleport-busy>
      <LoaderCircleIcon className="size-4 animate-spin" /> {children}
    </div>
  );
}

/** What moves, and the signed-in state move's opt-ins (each unchecked until chosen). */
function Options({ state, session, teleport }: { state: PickerState; session: TeleportSession; teleport: TeleportStore }) {
  const moves = state.entry?.moves ?? [];
  return (
    <div className="space-y-4 px-5 pb-4">
      <section className="rounded-lg border bg-card">
        <h3 className="border-b px-4 py-2 text-xs font-medium text-muted-foreground">Move</h3>
        <RadioGroup
          value={state.move ?? "app_only"}
          onValueChange={(v) => teleport.send({ type: "move", move: v as TeleportMove })}
          className="divide-y"
          aria-label="Move"
        >
          {moves.map((m) => (
            <label key={m} className="flex cursor-default items-center gap-3 px-4 py-2.5 text-[13px]" data-move={m}>
              <Radio.Root
                value={m}
                className="flex size-4 shrink-0 items-center justify-center rounded-full border border-input bg-card shadow-xs outline-none focus-visible:ring-2 focus-visible:ring-ring/60 data-checked:border-brand data-checked:bg-brand dark:bg-input/30"
              >
                <Radio.Indicator className="size-1.5 rounded-full bg-white" />
              </Radio.Root>
              {MOVE_LABEL[m]}
            </label>
          ))}
        </RadioGroup>
      </section>
      {session.frame.sensitive.length ? (
        <section className="divide-y rounded-lg border bg-card">
          {session.frame.sensitive.map((o) => (
            <label key={o.group} className="flex cursor-default items-start gap-3 px-4 py-2.5" data-sensitive={o.group} data-checked={o.checked || undefined}>
              <Checkbox
                className="mt-0.5"
                checked={o.checked}
                onCheckedChange={(value) => teleport.send({ type: "sensitive", group: o.group, value })}
              />
              <span className="min-w-0">
                <span className="block text-[13px]" data-sensitive-label>
                  {o.label}
                </span>
                <span className="block text-xs text-muted-foreground" data-sensitive-detail>
                  {o.detail}
                </span>
              </span>
            </label>
          ))}
        </section>
      ) : null}
      {state.files.length ? (
        <section className="divide-y rounded-lg border bg-card">
          {state.files.map((f) => (
            <div key={f} className="flex items-center justify-between gap-3 px-4 py-2 text-[13px]">
              <span className="min-w-0 truncate font-mono text-xs">{f}</span>
              <Button size="sm" variant="ghost" onClick={() => teleport.send({ type: "remove-file", path: f })}>
                Remove
              </Button>
            </div>
          ))}
        </section>
      ) : null}
    </div>
  );
}

/** The run: the bar, and under it the step it is on in words (the core's
 * `flow.status`, with bytes while they move), so the Keychain prompts that
 * come with reading a browser's cookies are expected. */
function Running({ session: s }: { session: TeleportSession }) {
  const permille = Math.round(s.frame.progress * 1000);
  return (
    <div className="px-5 py-4">
      <div
        className="h-1.5 overflow-hidden rounded-full bg-foreground/10"
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={permille / 10}
        data-teleport-progress={permille}
      >
        <div className="h-full rounded-full bg-brand transition-[width] duration-300" style={{ width: `${permille / 10}%` }} />
      </div>
      <p className="mt-2 min-h-[2.5em] text-[13px] text-muted-foreground" data-teleport-status>
        {s.frame.status ?? "\u00a0"}
      </p>
    </div>
  );
}

function Footer({ session: s, teleport }: { session: TeleportSession; teleport: TeleportStore }) {
  const step = s.state.step;
  const leaves = step === "consent" ? s.frame.review?.leavesText : null;
  return (
    <div className="flex shrink-0 items-center gap-2 border-t bg-muted/60 px-5 py-3">
      {leaves ? (
        <span className="text-xs text-muted-foreground" data-review-leaves>
          {leaves}
        </span>
      ) : null}
      <span className="flex-1" />
      {step !== "done" ? (
        <Button variant="outline" onClick={() => teleport.close()}>
          Cancel
        </Button>
      ) : null}
      {step === "pick" ? (
        <Button data-teleport-primary disabled={!s.primary.enabled} onClick={() => void teleport.activate()}>
          {s.primary.label}
        </Button>
      ) : step === "options" ? (
        <>
          <Button variant="outline" onClick={() => teleport.send({ type: "back" })}>
            Back
          </Button>
          <Button data-teleport-plan disabled={!s.frame.canPlan} onClick={() => void teleport.plan()}>
            Review
          </Button>
        </>
      ) : step === "consent" ? (
        <>
          <Button variant="outline" onClick={() => teleport.send({ type: "back" })}>
            Back
          </Button>
          <Button data-teleport-confirm disabled={!s.frame.review?.canConfirm} onClick={() => void teleport.confirm()}>
            Teleport
          </Button>
        </>
      ) : step === "error" ? (
        <Button variant="outline" onClick={() => teleport.send({ type: "back" })}>
          Back
        </Button>
      ) : step === "done" ? (
        <Button onClick={() => teleport.close()}>Done</Button>
      ) : null}
    </div>
  );
}
