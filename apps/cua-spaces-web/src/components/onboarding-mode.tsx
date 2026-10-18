// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useState } from "react";

import { useMachines, useThisMachine, type OnboardingHook, type OnboardingMode, type OnboardingView } from "@/bridge";
import { Button } from "@/components/ui/button";
import { JoinWithCodeDialog } from "@/components/join-with-code-dialog";
import { HostSetupForm } from "@/components/this-machine";
import { cn } from "@/lib/utils";

type HostSetup = ReturnType<typeof useThisMachine>;

/**
 * The This machine page's choice: the cards only select, and Continue (Return
 * too) moves on, like every other page. "Access other machines" is selected
 * to begin with, so Continue is always there. Continuing with Access other
 * machines finishes the page; with Set up this machine it opens host setup's
 * form (as the SwiftUI app does), and the page moves on once setup worked. A
 * machine already set up goes straight on.
 */
export function useModeChoice(flow: OnboardingHook, view: OnboardingView | null | undefined, host: HostSetup) {
  const [picked, setPicked] = useState<OnboardingMode | null>(null);
  const selected: OnboardingMode = picked ?? view?.preselectedMode ?? "client";
  const advance = () => {
    if (selected === "host" && !flow.hostConfigured) host.openForm();
    else flow.send({ type: "mode-chosen", mode: selected });
  };
  return { selected, select: setPicked, advance };
}

export type ModeChoice = ReturnType<typeof useModeChoice>;

/** The page's Continue button (not shown while host setup's form is open: it has its own). */
export function ModeContinue({ view, host, choice }: { view: OnboardingView; host: HostSetup; choice: ModeChoice }) {
  if (host.formView) return null;
  return (
    <Button size="lg" data-onboarding-primary="" data-mode-continue="" onClick={choice.advance}>
      {view.primaryLabel}
    </Button>
  );
}

export function ModeCard({ flow, view, host, choice }: { flow: OnboardingHook; view: OnboardingView; host: HostSetup; choice: ModeChoice }) {
  const { data: machines } = useMachines();
  const [joining, setJoining] = useState(false);
  if (host.formView) {
    const done = (ok: boolean) => {
      if (ok) flow.send({ type: "mode-chosen", mode: "host" });
      return ok;
    };
    return (
      <HostSetupForm
        view={host.formView}
        failure={host.setupError}
        progress={machines?.find((m) => m.current)?.hostProgress}
        onRetry={() => void host.retrySetUp().then(done)}
        host={{
          send: host.send,
          closeForm: host.closeForm,
          submit: async () => done(await host.submit()),
        }}
      />
    );
  }
  return (
    <div className="flex flex-col gap-2.5" role="radiogroup" aria-label={view.title}>
      {view.choices.map((c) => {
        const setUp = c.mode === "host" && !flow.hostConfigured;
        const on = c.mode === choice.selected;
        return (
          <button
            key={c.mode}
            type="button"
            role="radio"
            aria-checked={on}
            onClick={() => {
              choice.select(c.mode);
              // Return then continues, as on every other page.
              requestAnimationFrame(() => document.querySelector<HTMLElement>("[data-mode-continue]")?.focus());
            }}
            data-testid={`onboarding-${c.mode}`}
            className={cn(
              "rounded-xl border bg-card px-4 py-3.5 text-left shadow-xs outline-none transition-[border-color,box-shadow]",
              "hover:border-foreground/20 focus-visible:border-brand focus-visible:ring-3 focus-visible:ring-brand/25",
              on && "border-foreground/60 ring-1 ring-foreground/30",
            )}
          >
            <div className="text-[13px] font-semibold">{c.label}</div>
            {setUp ? (
              <div className="mt-0.5 text-xs text-muted-foreground">Next: name this machine and choose who can reach it.</div>
            ) : null}
          </button>
        );
      })}
      {flow.hostConfigured ? null : (
        <>
          <button
            type="button"
            onClick={() => setJoining(true)}
            data-testid="onboarding-join-with-code"
            className="self-start px-1 pt-1 text-xs text-muted-foreground outline-none hover:text-foreground focus-visible:text-foreground"
          >
            Have a code from another device? Join with a code
          </button>
          <JoinWithCodeDialog open={joining} onOpenChange={setJoining} />
        </>
      )}
    </div>
  );
}
