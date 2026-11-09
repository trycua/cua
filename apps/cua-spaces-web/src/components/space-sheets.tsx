// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { lazy, Suspense } from "react";

import { useShareSheet, useTeleport } from "@/bridge";
import { ProviderSheetHost } from "@/components/settings/connections/provider-sheet";

const TeleportDialog = lazy(() => import("./teleport/teleport-dialog"));
const ShareSheet = lazy(() => import("./share-sheet"));

/** The sheets a Space opens from anywhere: "Teleport an app", "Share" and
 * a provider's Connect (Settings, an app's page, or a Space's "isn't connected yet").
 * Each loads when first opened. */
export function SpaceSheets() {
  const teleport = useTeleport();
  const share = useShareSheet();
  return (
    <Suspense fallback={null}>
      {teleport.session ? <TeleportDialog /> : null}
      {share.session ? <ShareSheet /> : null}
      <ProviderSheetHost />
    </Suspense>
  );
}
