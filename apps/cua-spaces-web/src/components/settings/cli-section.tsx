// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Settings → Command line: the app's bundled CLI, as the first run's
 * "Install the Cua CLI" screen words it (`firstRun.cli`): where it is, or
 * the exact file Install writes, and Install. Shown where the host can read
 * the install plan (`cli.plan`).
 */

import { firstRunCliView, useBridge, useCliInstall, useSettings } from "@/bridge";
import { SettingsGroup, SettingsRow } from "@/components/settings-group";
import { Button } from "@/components/ui/button";

/** "~/.local/bin, in ~/.zshrc" as a sentence. */
function pathLine(value: string): string {
  const [dir, profile] = value.split(", in ");
  return profile ? `Install also adds ${dir} to your PATH in ${profile}.` : `Install also adds ${dir} to your PATH.`;
}

export function CliSettingsSection() {
  const { core } = useBridge();
  const cli = useCliInstall();
  const name = useSettings().data?.cli?.name ?? null;
  if (!cli.state.plan) return null;
  const view = firstRunCliView(core, { cli: cli.state, cliName: name });
  if (!view) return null;
  const install = view.primary?.id === "install" ? view.primary : null;
  const pathFact = view.facts.find((f) => f.label === "Adds to PATH");
  return (
    <SettingsGroup title="Terminal">
      <div data-settings-cli={view.installed ? "installed" : "missing"}>
        <SettingsRow
          label="Cua CLI"
          description={
            <>
              {view.status}
              {view.error ? <span className="mt-0.5 block text-destructive">{view.error}</span> : null}
              {!view.installed && pathFact ? <span className="mt-0.5 block">{pathLine(pathFact.value)}</span> : null}
            </>
          }
          control={
            install ? (
              <Button size="sm" variant="outline" disabled={view.busy} data-settings-cli-install="" onClick={() => void cli.install()}>
                {view.busy ? "Installing…" : install.label}
              </Button>
            ) : view.tryCommand ? (
              <code className="font-mono text-xs text-muted-foreground">{view.tryCommand}</code>
            ) : null
          }
        />
      </div>
    </SettingsGroup>
  );
}
