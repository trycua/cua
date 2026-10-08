// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute } from "@tanstack/react-router";
import { InfoIcon } from "lucide-react";
import { useState } from "react";

import { useAbout, useSession, type AboutLink, type AboutUpdates, type UpdateChannel } from "@/bridge";
import { CuaLogo } from "@/components/cua-logo";
import { NoticesDialog } from "@/components/settings/notices-dialog";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Select } from "@/components/ui/select";
import { toastError } from "@/components/ui/toast";
import { Tooltip } from "@/components/ui/tooltip";

export const Route = createFileRoute("/settings/about")({ component: AboutPage });

/**
 * Settings, About: the name and version, the links, the copyright, then the
 * update controls where the app updates itself. Everything shown is the
 * app core's `about::view`, as in the SwiftUI app's `AboutSettingsView`.
 */
function AboutPage() {
  const { data, setAbout, checkNow } = useAbout();
  const { openExternal } = useSession();
  const [notices, setNotices] = useState(false);
  const view = data?.view;
  if (!view) return null;

  const open = (link: AboutLink) => {
    if (link.url) void openExternal(link.url).catch(toastError(`Couldn't open ${link.label}`));
    else setNotices(true);
  };

  return (
    <section className="rounded-xl border bg-card px-8 py-8 shadow-xs" data-about>
      <div className="flex flex-col items-center text-center">
        <CuaLogo className="size-14" />
        <h2 className="mt-4 text-[17px] font-semibold" data-about-title>
          {view.title}
        </h2>
        <p className="mt-1 text-[13px] text-muted-foreground select-text" data-about-version>
          {view.versionLine}
        </p>
        <div className="mt-5 flex flex-col items-center gap-1.5">
          {view.links.map((link) => (
            <button
              key={link.id}
              type="button"
              data-about-link={link.id}
              onClick={() => open(link)}
              className="text-[13px] text-brand-strong outline-none hover:underline focus-visible:underline"
            >
              {link.label}
            </button>
          ))}
        </div>
        <p className="mt-5 text-xs text-muted-foreground" data-about-copyright>
          {view.copyright}
        </p>
      </div>
      {view.updates ? (
        <UpdateControls
          updates={view.updates}
          canChange={data.canChange}
          onChange={(patch) => void setAbout(patch).catch(toastError("Couldn't change the update settings"))}
          onCheck={() => void checkNow().catch(toastError("Couldn't check for updates"))}
        />
      ) : null}
      <NoticesDialog open={notices} onOpenChange={setNotices} />
    </section>
  );
}

function UpdateControls({
  updates: u,
  canChange,
  onChange,
  onCheck,
}: {
  updates: AboutUpdates;
  canChange: boolean;
  onChange: (patch: { autoCheck?: boolean; autoInstall?: boolean; channel?: UpdateChannel }) => void;
  onCheck: () => void;
}) {
  const channel = (u.channels.find((c) => c.active)?.id ?? "stable") as UpdateChannel;
  return (
    <div className="mt-7 border-t pt-6" data-about-updates>
      <div className="mx-auto grid w-fit grid-cols-[auto_auto] items-center gap-x-3 gap-y-3">
        <span />
        <label className="flex items-center gap-2 text-[13px]">
          <Checkbox checked={u.autoCheck} disabled={!canChange} onCheckedChange={(on) => onChange({ autoCheck: on })} data-about-auto-check />
          {u.autoCheckLabel}
        </label>
        <span />
        <label className="flex items-center gap-2 text-[13px]">
          <Checkbox
            checked={u.autoInstall}
            disabled={!canChange || !u.autoInstallEnabled}
            onCheckedChange={(on) => onChange({ autoInstall: on })}
            data-about-auto-install
          />
          <span className={u.autoInstallEnabled ? undefined : "text-muted-foreground"}>{u.autoInstallLabel}</span>
        </label>
        <span className="text-right text-[13px]">{u.channelLabel}</span>
        <span className="flex items-center gap-2">
          <Select
            aria-label={u.channelLabel}
            className="min-w-28"
            value={channel}
            disabled={!canChange}
            options={u.channels.map((c) => ({ value: c.id as UpdateChannel, label: c.label }))}
            onValueChange={(v) => onChange({ channel: v })}
          />
          <Tooltip content={<span className="block max-w-64">{u.channelHelp}</span>}>
            <InfoIcon className="size-4 text-muted-foreground" aria-label={u.channelHelp} />
          </Tooltip>
        </span>
        <span />
        <span className="flex items-center gap-3">
          <Button size="sm" variant="outline" disabled={!u.checkEnabled || !canChange} onClick={onCheck} data-about-check>
            {u.checkLabel}
          </Button>
          <span className="text-xs text-muted-foreground" data-about-last-check>
            {u.lastCheck}
          </span>
        </span>
      </div>
    </div>
  );
}
