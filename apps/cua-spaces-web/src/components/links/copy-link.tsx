// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Link } from "@tanstack/react-router";
import { useState } from "react";

import { needsRelaySetup, useLinks } from "@/bridge";
import { SettingsGroup } from "@/components/settings-group";
import { Button } from "@/components/ui/button";

type Status = { kind: "idle" } | { kind: "busy" } | { kind: "copied" } | { kind: "error"; message: string };

/** The host's words without the error class (`unsupported: ...`). */
function plain(message: string): string {
  return message.replace(/^(unsupported|invalid argument|not found|internal): /i, "");
}

/** The failure line: the host's words, and the fix when the machine is not on the relay. */
export function LinkError({ message }: { message: string }) {
  const setup = needsRelaySetup(message);
  return (
    <p data-link-error="" className="px-4 pb-3 text-[13px] text-destructive">
      {setup ? "This machine is not on your cua.ai relay yet, so a link would only open here." : plain(message)}
      {setup ? (
        <>
          {" "}
          <Link to="/machines" className="underline" data-link-setup="">
            Set up this machine first
          </Link>
        </>
      ) : null}
    </p>
  );
}

/**
 * "Copy link" for a port in a Space: shares `space:<space>:<port>` (private:
 * your signed-in devices) and puts the link on the clipboard. Nothing shows
 * where the host has no links.
 */
export function SpacePortLink({ spaceId, title = "Link to a port" }: { spaceId: string; title?: string }) {
  const { unsupported, createLink } = useLinks({ load: false });
  const [port, setPort] = useState("");
  const [status, setStatus] = useState<Status>({ kind: "idle" });
  if (unsupported) return null;
  const n = Number(port);
  const valid = Number.isInteger(n) && n > 0 && n < 65536;

  const copy = async () => {
    setStatus({ kind: "busy" });
    try {
      const link = await createLink({ target: `space:${spaceId}:${n}` });
      await navigator.clipboard.writeText(link.url);
      setStatus({ kind: "copied" });
    } catch (e) {
      setStatus({ kind: "error", message: e instanceof Error ? e.message : String(e) });
    }
  };

  return (
    <SettingsGroup title={title}>
      <form
        className="flex min-h-12 items-center justify-between gap-6 px-4 py-2.5"
        onSubmit={(e) => {
          e.preventDefault();
          if (valid) void copy();
        }}
      >
        <label htmlFor="link-port" className="text-[13px]">
          Port
        </label>
        <div className="flex items-center gap-2">
          <input
            id="link-port"
            inputMode="numeric"
            placeholder="3000"
            value={port}
            onChange={(e) => {
              setPort(e.target.value.replace(/\D/g, "").slice(0, 5));
              setStatus({ kind: "idle" });
            }}
            className="h-8 w-20 rounded-lg border bg-background px-2 text-right text-[13px] outline-none focus-visible:ring-2 focus-visible:ring-ring/60"
          />
          <Button type="submit" variant="outline" disabled={!valid || status.kind === "busy"} data-copy-link="">
            {status.kind === "copied" ? "Link copied" : "Copy link"}
          </Button>
        </div>
      </form>
      {status.kind === "error" ? <LinkError message={status.message} /> : null}
    </SettingsGroup>
  );
}
