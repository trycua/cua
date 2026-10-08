// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useNavigate } from "@tanstack/react-router";

import { createFailedText, useBridge, useNewSpaceWizard, useSpaces } from "@/bridge";
import { toast, toastError } from "@/components/ui/toast";

/**
 * Opens New Space. Where the wizard can't run (no app core, or a host that
 * keeps New Space native), starts the host's own create instead: the
 * SwiftUI app opens its sheet, the others create a Space with the defaults.
 */
export function useNewSpace(): () => void {
  const wizard = useNewSpaceWizard();
  const { createSpace } = useSpaces();
  return () => {
    if (wizard.offered) return wizard.show();
    void createSpace()
      .then((s) => toast(`${s.name} is ready`, { type: "success" }))
      .catch(toastError("Couldn't create the Space"));
  };
}

/**
 * Create Space: the wizard closes, the Spaces page shows the new tile with
 * its progress (the bridge's creates flow), and a toast says when it is
 * ready or why it failed. "Open when ready" opens its desktop.
 */
export function useCreateFromWizard(): () => void {
  const wizard = useNewSpaceWizard();
  const { core } = useBridge();
  const { openSpace } = useSpaces();
  const navigate = useNavigate();
  return () => {
    const openDesktop = wizard.view?.plan.openDesktop ?? false;
    const created = wizard.create();
    if (!created) return;
    void navigate({ to: "/spaces" });
    created
      .then((s) => {
        toast(`${s.name} is ready`, { type: "success" });
        if (openDesktop) void openSpace(s.id).catch(() => {});
      })
      .catch((e: unknown) => {
        const message = e instanceof Error ? e.message : String(e);
        // A cancelled create just goes away.
        if (!message.startsWith("cancelled")) toast(createFailedText(core, e), { type: "error" });
      });
  };
}
