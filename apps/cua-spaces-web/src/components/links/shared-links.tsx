// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useState } from "react";

import { linkEnds, useLinks, type ShareLink } from "@/bridge";
import { SettingsGroup } from "@/components/settings-group";
import { portOf, shareFoot, spaceOf } from "@/components/links/tcp-words";
import { WaitingShare } from "@/components/links/waiting-share";
import { Button } from "@/components/ui/button";
import { toastError } from "@/components/ui/toast";

const ACCESS: Record<ShareLink["access"], string> = { private: "You", team: "Team", public: "Anyone" };

/** Settings: the links this machine is serving, each with Copy link and Stop. Hidden where the host has no links. */
export function SharedLinks() {
  const { links, unsupported, stopLink, stopAll, approveLink, denyLink } = useLinks();
  const [copied, setCopied] = useState<string | null>(null);
  if (unsupported) return null;

  // A public TCP share copies its address (what people type), not a link.
  const copy = (l: ShareLink) =>
    navigator.clipboard.writeText(l.tcpConnectWith ?? l.url).then(() => {
      setCopied(l.id);
      setTimeout(() => setCopied((c) => (c === l.id ? null : c)), 2000);
    }, toastError("Could not copy the link"));

  return (
    <SettingsGroup className="mb-7" title="Shared links">
      {links.length === 0 ? (
        <p data-links-empty="" className="px-4 py-3 text-[13px] text-muted-foreground">
          Nothing is shared.
        </p>
      ) : (
        links.map((l) =>
          l.waiting ? (
            <div key={l.id} data-link={l.id} data-link-waiting={l.waiting} className="flex flex-col gap-2.5 px-4 py-3">
              <div className="min-w-0">
                <div className="truncate text-[13px]">
                  {spaceOf(l)} port {portOf(l)}
                </div>
                <div className="text-xs text-muted-foreground">Waiting for you</div>
              </div>
              <WaitingShare link={l} onApprove={(always) => approveLink(l.id, always)} onDeny={() => denyLink(l.id)} />
            </div>
          ) : (
            <div key={l.id} data-link={l.id} className="flex min-h-12 items-center justify-between gap-4 px-4 py-2">
              <div className="min-w-0">
                {l.tcpConnectWith ? (
                  <>
                    <div className="font-mono text-[13px] break-all" data-link-address="">
                      {l.tcpConnectWith}
                    </div>
                    {l.routeLine ? (
                      <div className="truncate text-xs" data-link-route={l.route ?? ""}>
                        {l.routeLine}
                      </div>
                    ) : null}
                    <div className="truncate text-xs text-muted-foreground">
                      {spaceOf(l)} port {portOf(l)}. {shareFoot(l)}
                    </div>
                  </>
                ) : (
                  <>
                    <div className="truncate text-[13px]">{l.name ?? l.target}</div>
                    <div className="truncate text-xs text-muted-foreground">
                      {ACCESS[l.access]}, {l.expiresAtUnix === null ? "until stopped" : `ends in ${linkEnds(l)}`}
                    </div>
                  </>
                )}
              </div>
              <div className="flex shrink-0 gap-2">
                <Button variant="outline" onClick={() => void copy(l)} data-link-copy="">
                  {copied === l.id ? (l.tcpConnectWith ? "Address copied" : "Link copied") : l.tcpConnectWith ? "Copy address" : "Copy link"}
                </Button>
                <Button variant="outline" onClick={() => void stopLink(l.id).catch(toastError("Could not stop the link"))} data-link-stop="">
                  Stop
                </Button>
              </div>
            </div>
          ),
        )
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
