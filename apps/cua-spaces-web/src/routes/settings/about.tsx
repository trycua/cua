// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { ChevronRightIcon } from "lucide-react";
import { useState } from "react";

import { useAbout, useBridge, useSession, useSettings, type AboutLink, type AboutUpdates } from "@/bridge";
import { writeSavedOnboarding } from "@/bridge/onboarding";
import { aboutView } from "@/bridge/settings-derive";
import { SettingsGroup, SettingsRow } from "@/components/settings-group";
import { NoticesDialog } from "@/components/settings/notices-dialog";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { toast, toastError } from "@/components/ui/toast";
import { lastChecked } from "@/lib/settings-copy";

export const Route = createFileRoute("/settings/about")({ component: AboutPage });

/** What the update controls say in a build that can't update itself. */
const UPDATES_UNAVAILABLE = "Updates aren’t available in this build";

/**
 * Settings, About: the name and version (with Copy version), the links, then
 * the update controls where the app updates itself, and the welcome again.
 * Everything shown is the app core's `about::view`; the update channel
 * (Beta) lives in Advanced. The Electron app keeps the controls in a build
 * with no update feed (a local build), greyed out, and says so.
 */
function AboutPage() {
  const { data, setAbout, checkNow } = useAbout();
  const { openExternal } = useSession();
  const { mode, core } = useBridge();
  const navigate = useNavigate();
  const { chooseSetting } = useSettings();
  const [notices, setNotices] = useState(false);
  const view = data?.view;
  if (!view) return null;
  const unavailable = !view.updates && mode === "electron" ? (aboutView(core, { ...data.input, updater: true })?.updates ?? null) : null;

  const open = (link: AboutLink) => {
    if (link.url) void openExternal(link.url).catch(toastError(`Couldn't open ${link.label}`));
    else setNotices(true);
  };
  const copyVersion = () => {
    void navigator.clipboard?.writeText(view.versionLine.replace(/^Version /, "")).then(() => toast("Version copied"));
  };
  const showWelcome = () => {
    // The SwiftUI app shows its own welcome window; elsewhere it is this UI's first run, from the start.
    if (mode === "webkit") return void chooseSetting("welcome", "show").catch(toastError("Couldn't show the welcome"));
    writeSavedOnboarding(null);
    void navigate({ to: "/onboarding" });
  };
  const updates = view.updates ?? (unavailable ? { ...unavailable, checkEnabled: false, lastCheck: UPDATES_UNAVAILABLE } : null);

  return (
    <div data-about="">
      <SettingsGroup
        title="Cua Spaces"
        footer={<span data-about-copyright>{view.copyright}</span>}
      >
        <SettingsRow
          label={<span data-about-title>{view.title}</span>}
          description={
            <span className="select-text" data-about-version>
              {view.versionLine}
            </span>
          }
          control={
            <Button size="sm" variant="outline" onClick={copyVersion} data-about-copy-version>
              Copy version
            </Button>
          }
        />
        {view.links.map((link) => (
          <button
            key={link.id}
            type="button"
            data-about-link={link.id}
            onClick={() => open(link)}
            className="flex min-h-11 w-full items-center justify-between gap-6 px-4 py-2.5 text-left text-[13px] outline-none hover:bg-accent/50 focus-visible:bg-accent/50"
          >
            <span data-about-link-label="">{link.label}</span>
            <span className="flex items-center text-xs text-muted-foreground">
              Open <ChevronRightIcon className="size-3.5" />
            </span>
          </button>
        ))}
      </SettingsGroup>
      <SettingsGroup title="Updates">
        {updates ? (
          <div data-about-updates="" className="divide-y" {...(unavailable ? { "data-about-unavailable": "" } : {})}>
            <UpdateRows
              updates={updates}
              canChange={data.canChange && !unavailable}
              onChange={(on) => void setAbout({ autoCheck: on, autoInstall: on }).catch(toastError("Couldn't change the update settings"))}
              onCheck={() => void checkNow().catch(toastError("Couldn't check for updates"))}
            />
          </div>
        ) : null}
        <SettingsRow label="Show the welcome again" control={<Button size="sm" variant="outline" onClick={showWelcome} data-about-welcome="">Show</Button>} />
      </SettingsGroup>
      <NoticesDialog open={notices} onOpenChange={setNotices} />
    </div>
  );
}

/** "Keep Cua Spaces up to date": one switch for checking, downloading and installing when you quit. */
function UpdateRows({ updates: u, canChange, onChange, onCheck }: { updates: AboutUpdates; canChange: boolean; onChange: (on: boolean) => void; onCheck: () => void }) {
  return (
    <>
      <SettingsRow
        label="Keep Cua Spaces up to date"
        description="Updates download in the background and install when you quit."
        htmlFor="about-auto-update"
        control={<Switch id="about-auto-update" data-about-auto-check="" checked={u.autoCheck} disabled={!canChange} onCheckedChange={onChange} />}
      />
      <SettingsRow
        label={<span data-about-last-check>{lastChecked(u.lastCheck)}</span>}
        control={
          <Button size="sm" variant="outline" disabled={!u.checkEnabled || !canChange} onClick={onCheck} data-about-check>
            Check now
          </Button>
        }
      />
    </>
  );
}
