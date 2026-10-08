// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute } from "@tanstack/react-router";

import { useExperiments } from "@/bridge";
import { CoreSettingsSection } from "@/components/settings/core-section";
import { toastError } from "@/components/ui/toast";

export const Route = createFileRoute("/settings/experiments")({ component: ExperimentsPage });

/**
 * Settings, Experiments: one switch per experiment with its one line, all
 * off until turned on (`experiments::page`). Off only hides an experiment's
 * entry points; nothing set up is undone.
 */
function ExperimentsPage() {
  const { data, unsupported, isLoading, chooseExperiment } = useExperiments();
  if (unsupported) return <p className="px-1 text-[13px] text-muted-foreground">Experiments are not available in this app yet.</p>;
  if (isLoading || !data?.page) return null;
  return data.page.sections.map((section) => (
    <CoreSettingsSection
      key={section.id}
      section={section}
      foldNotes
      onChoose={(row, option) => void chooseExperiment(row.id, option).catch(toastError("Couldn't change the experiment"))}
    />
  ));
}
