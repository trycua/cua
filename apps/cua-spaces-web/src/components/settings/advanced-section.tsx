// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { ChevronDownIcon, ChevronRightIcon } from "lucide-react";
import { useState } from "react";

import { useMachines, useSession, useThisMachine } from "@/bridge";
import { HostSetupForm } from "@/components/this-machine";
import { Button } from "@/components/ui/button";

/** The host form's profile for your own relay or Direct (`host::SELF_HOSTED_PROFILE`). */
const SELF_HOSTED = "self-hosted";

/**
 * Settings → Advanced, collapsed: the only place that offers your own relay
 * or a direct connection. Setup, first run and This machine offer Cua's
 * relay alone. The form is host setup's own, with Advanced open.
 */
export function AdvancedSection() {
  const [open, setOpen] = useState(false);
  const { data: session } = useSession();
  const { data: machines } = useMachines();
  const host = useThisMachine(session?.identity ?? null);
  const form = host.formView?.advancedOpen ? host.formView : null;

  return (
    <section className="mb-7" data-settings-advanced={open ? "open" : "closed"}>
      <button
        type="button"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
        className="mb-2 flex cursor-default items-center gap-1 px-1 text-xs font-semibold text-muted-foreground outline-none hover:text-foreground focus-visible:text-foreground"
      >
        {open ? <ChevronDownIcon className="size-3.5" /> : <ChevronRightIcon className="size-3.5" />}
        Advanced
      </button>
      {open ? (
        form ? (
          <HostSetupForm
            view={form}
            host={host}
            failure={host.setupError}
            progress={machines?.find((m) => m.current)?.hostProgress}
            onRetry={() => void host.retrySetUp()}
          />
        ) : (
          <div className="overflow-hidden rounded-xl border bg-card shadow-xs">
            <div className="flex min-h-12 items-center justify-between gap-6 px-4 py-2.5" data-self-hosting="">
              <div className="min-w-0">
                <div className="text-[13px]">Your own relay</div>
                <p className="mt-0.5 text-xs text-muted-foreground">For self-hosting teams. Connect this computer through a relay you run, or directly.</p>
              </div>
              <Button size="sm" variant="outline" disabled={host.busy} onClick={() => host.openForm(SELF_HOSTED)}>
                Set up…
              </Button>
            </div>
          </div>
        )
      ) : null}
    </section>
  );
}
