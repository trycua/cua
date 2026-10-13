// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useMachines, useThisMachine, type OnboardingHook, type OnboardingView } from "@/bridge";
import { HostSetupForm } from "@/components/this-machine";
import { cn } from "@/lib/utils";

type HostSetup = ReturnType<typeof useThisMachine>;

/**
 * Access other machines finishes the page. Set up this machine opens host
 * setup's form here (as the SwiftUI app does), with the permissions it
 * needs under it; the page moves on once setup worked. A machine already
 * set up goes straight on.
 */
export function ModeCard({ flow, view, host }: { flow: OnboardingHook; view: OnboardingView; host: HostSetup }) {
  const { data: machines } = useMachines();
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
    <div className="flex flex-col gap-2.5">
      {view.choices.map((choice) => {
        const setUp = choice.mode === "host" && !flow.hostConfigured;
        return (
          <button
            key={choice.mode}
            type="button"
            onClick={() => (setUp ? host.openForm() : flow.send({ type: "mode-chosen", mode: choice.mode }))}
            data-testid={`onboarding-${choice.mode}`}
            // Return on the page picks the preselected choice.
            data-onboarding-primary={choice.preselected ? "" : undefined}
            className={cn(
              "rounded-xl border bg-card px-4 py-3.5 text-left shadow-xs outline-none transition-[border-color,box-shadow]",
              "hover:border-foreground/20 focus-visible:border-brand focus-visible:ring-3 focus-visible:ring-brand/25",
              choice.preselected && "border-foreground/40",
            )}
          >
            <div className="text-[13px] font-semibold">{choice.label}</div>
            {setUp ? (
              <div className="mt-0.5 text-xs text-muted-foreground">Next: name this machine and choose who can reach it.</div>
            ) : null}
          </button>
        );
      })}
    </div>
  );
}
