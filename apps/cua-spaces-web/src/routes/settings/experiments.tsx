// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute } from "@tanstack/react-router";

import { useBridge, useExperiments } from "@/bridge";
import { CoreSettingsSection } from "@/components/settings/core-section";
import { toastError } from "@/components/ui/toast";

export const Route = createFileRoute("/settings/experiments")({ component: ExperimentsPage });

/** "New UI (preview)" opens the SwiftUI app's web window: the Electron app is that UI, so it has no such switch. */
const WEB_UI_ROWS = new Set(["experiment:web_ui", "experiment:web_ui-note"]);

/**
 * Settings, Experiments: one switch per experiment with its one line, all
 * off until turned on (`experiments::page`). Off only hides an experiment's
 * entry points; nothing set up is undone.
 */
function ExperimentsPage() {
  const { data, unsupported, isLoading, chooseExperiment } = useExperiments();
  const { mode } = useBridge();
  if (unsupported) return <p className="px-1 text-[13px] text-muted-foreground">Experiments are not available in this app yet.</p>;
  if (isLoading || !data?.page) return null;
  const shown = (id: string) => mode !== "electron" || !WEB_UI_ROWS.has(id);
  return data.page.sections.map((section) => (
    <CoreSettingsSection
      key={section.id}
      section={{ ...section, rows: section.rows.filter((r) => shown(r.id)) }}
      foldNotes
      onChoose={(row, option) => void chooseExperiment(row.id, option).catch(toastError("Couldn't change the experiment"))}
    />
  ));
}
