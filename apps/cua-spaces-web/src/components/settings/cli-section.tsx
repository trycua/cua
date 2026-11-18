// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Settings → Terminal: the app's bundled `cua`, as the first run's
 * "Use Cua from the Terminal" screen words it (`firstRun.cli`): where it is, or
 * the exact file Install writes, and Install. Shown where the host can read
 * the install plan (`cli.plan`).
 */

import { firstRunCliView, useBridge, useCliInstall, useSettings } from "@/bridge";
import { SettingsGroup, SettingsRow } from "@/components/settings-group";
import { Button } from "@/components/ui/button";
import { useQuiet } from "@/hooks/use-quiet";
import { useT } from "@/i18n";

/** "~/.local/bin, in ~/.zshrc" as a sentence. */
function pathLine(value: string, say: ReturnType<typeof useT>): string {
  const [dir, profile] = value.split(", in ");
  return profile ? say("settings.cli.path_profile", { dir, profile }) : say("settings.cli.path", { dir });
}

export function CliSettingsSection() {
  const t = useT();
  const { core } = useBridge();
  const cli = useCliInstall();
  const name = useSettings().data?.cli?.name ?? null;
  // The plan is a host read that answers after the page draws: hold its room until it has, so the page does not jump (CUA-1951).
  const settled = useQuiet(`${cli.state.plan ? 1 : 0}|${name ? 1 : 0}`);
  if (!settled) return <div aria-hidden className="mb-8 h-[84px]" />;
  if (!cli.state.plan) return null;
  const view = firstRunCliView(core, { cli: cli.state, cliName: name });
  if (!view) return null;
  const install = view.primary?.id === "install" ? view.primary : null;
  const pathFact = view.facts.find((f) => f.label === "Adds to PATH");
  return (
    <SettingsGroup title={t("settings.cli.title")}>
      <div data-settings-cli={view.installed ? "installed" : "missing"}>
        <SettingsRow
          label={t("settings.cli.label")}
          description={
            <>
              {view.status}
              {view.error ? <span className="mt-0.5 block text-destructive">{view.error}</span> : null}
              {!view.installed && pathFact ? <span className="mt-0.5 block">{pathLine(pathFact.value, t)}</span> : null}
            </>
          }
          control={
            install ? (
              <Button size="sm" variant="outline" disabled={view.busy} data-settings-cli-install="" onClick={() => void cli.install()}>
                {view.busy ? t("settings.cli.installing") : install.label}
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
