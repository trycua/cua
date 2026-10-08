// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect } from "react";

import { useSettings, type SettingsSection } from "@/bridge";
import { CoreSettingsSection } from "@/components/settings/core-section";
import { toastError } from "@/components/ui/toast";

/**
 * Settings, AI agents, as the SwiftUI app's (the core's `agents` section):
 * each coding agent on this machine with what cua set up for it ("skills
 * 0/5, MCP not configured") and Configure or Remove, and "Configure all
 * detected agents" in the header. The agents are read when this shows
 * (`agents.setup`); a row's button is the host's `press` (`settings.choose`),
 * the header `agents.configure`; "working…" and "Configuring…" show while
 * they run.
 */
export function AgentsSettingsSection({ section }: { section: SettingsSection }) {
  const { loadAgentRows, configureAllAgents, pressAgentRow } = useSettings();

  useEffect(() => {
    void loadAgentRows().catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps -- once per showing
  }, []);

  return (
    <CoreSettingsSection
      section={section}
      onHeaderButton={() => void configureAllAgents().catch(toastError("Couldn't configure the agents"))}
      onPress={(row) => void pressAgentRow(row.id).catch(toastError(`Couldn't change ${row.label}`))}
    />
  );
}
