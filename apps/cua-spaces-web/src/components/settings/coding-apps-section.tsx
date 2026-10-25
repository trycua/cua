// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { lazy, Suspense } from "react";

import { useSettings } from "@/bridge";
import { SettingsGroup } from "@/components/settings-group";
import { AgentsSettingsSection } from "@/components/settings/agents-section";

// Its miniature and setup load with the card.
const DriverSettingsCard = lazy(() => import("@/components/volume/driver-card").then((m) => ({ default: m.DriverSettingsCard })));

/**
 * Settings, Connected apps, "Coding apps": the coding apps on this computer
 * and whether Cua is set up in each one.
 */
export function CodingAppsSection() {
  const section = useSettings().data?.page?.sections.find((s) => s.id === "agents");
  return (
    <>
      {section ? <AgentsSettingsSection section={section} /> : null}
      <SettingsGroup title="Background computer use">
        <Suspense fallback={<div className="h-48" />}>
          <DriverSettingsCard />
        </Suspense>
      </SettingsGroup>
    </>
  );
}
