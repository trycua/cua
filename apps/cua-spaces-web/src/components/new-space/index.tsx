// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { lazy, Suspense } from "react";

import { useConnectCloud, useNewSpaceRequests, useNewSpaceWizard } from "@/bridge";

const NewSpaceDialog = lazy(() => import("./new-space-dialog").then((m) => ({ default: m.NewSpaceDialog })));

/** New Space and Connect a cloud, loaded the first time either opens. */
export function NewSpace() {
  const { open } = useNewSpaceWizard();
  const connect = useConnectCloud();
  useNewSpaceRequests();
  if (!open && !connect.open) return null;
  return (
    <Suspense fallback={null}>
      <NewSpaceDialog />
    </Suspense>
  );
}
