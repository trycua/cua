// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useState } from "react";

import { linkEnds, useLinks, type ShareLink } from "@/bridge";
import { SettingsGroup } from "@/components/settings-group";
import { Button } from "@/components/ui/button";
import { toastError } from "@/components/ui/toast";

const ACCESS: Record<ShareLink["access"], string> = { private: "You", team: "Team", public: "Anyone" };

/** Settings: the links this machine is serving, each with Copy link and Stop. Hidden where the host has no links. */
export function SharedLinks() {
  const { links, unsupported, stopLink, stopAll } = useLinks();
  const [copied, setCopied] = useState<string | null>(null);
  if (unsupported) return null;

  const copy = (l: ShareLink) =>
    navigator.clipboard.writeText(l.url).then(() => {
      setCopied(l.id);
      setTimeout(() => setCopied((c) => (c === l.id ? null : c)), 2000);
    }, toastError("Could not copy the link"));

  return (
    <SettingsGroup title="Shared links">
      {links.length === 0 ? (
        <p data-links-empty="" className="px-4 py-3 text-[13px] text-muted-foreground">
          Nothing is shared.
        </p>
      ) : (
        links.map((l) => (
          <div key={l.id} data-link={l.id} className="flex min-h-12 items-center justify-between gap-4 px-4 py-2">
            <div className="min-w-0">
              <div className="truncate text-[13px]">{l.name ?? l.target}</div>
              <div className="truncate text-xs text-muted-foreground">
                {ACCESS[l.access]} · {linkEnds(l)}
              </div>
            </div>
            <div className="flex shrink-0 gap-2">
              <Button variant="outline" onClick={() => void copy(l)} data-link-copy="">
                {copied === l.id ? "Link copied" : "Copy link"}
              </Button>
              <Button variant="outline" onClick={() => void stopLink(l.id).catch(toastError("Could not stop the link"))} data-link-stop="">
                Stop
              </Button>
            </div>
          </div>
        ))
      )}
      {links.length > 1 ? (
        <div className="flex justify-end px-4 py-2">
          <Button variant="outline" onClick={() => void stopAll().catch(toastError("Could not stop the links"))} data-link-stop-all="">
            Stop all
          </Button>
        </div>
      ) : null}
    </SettingsGroup>
  );
}
