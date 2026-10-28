// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useCallback, useEffect, useRef, useState } from "react";

import { isUnsupported, useBridge } from "@/bridge";
import type { RepoCacheChange, RepoCacheView } from "@/bridge/contracts/settings";
import { SettingsGroup, SettingsRow } from "@/components/settings-group";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { toastError } from "@/components/ui/toast";

/** How often what the cache holds is read again while Settings is open. */
const REFRESH_MS = 15_000;

/**
 * Settings, General: "Repo and dependency cache". A switch (on by default), the most disk it may use
 * (empty is automatic), "Clear cache", and in plain words what it holds and what the last week saved.
 * The host owns all of it (the repo cache of the Cua daemon); a host without one draws nothing.
 */
export function RepoCacheGroup() {
  const { data: adapter } = useBridge();
  const [view, setView] = useState<RepoCacheView | null>(null);
  const [busy, setBusy] = useState(false);
  const shown = view && view.sizeSetting !== "auto" ? view.sizeSetting : "";
  const [draft, setDraft] = useState(shown);
  const latest = useRef(shown);
  latest.current = shown;
  useEffect(() => setDraft(shown), [shown]);

  const read = useCallback(async () => {
    if (!adapter) return;
    try {
      setView(await adapter.call("repoCache.get", {}));
    } catch (e) {
      // A host without a repo cache has no row; any other failure keeps what is shown.
      if (isUnsupported(e)) setView(null);
    }
  }, [adapter]);

  useEffect(() => {
    void read();
    const t = setInterval(() => void read(), REFRESH_MS);
    return () => clearInterval(t);
  }, [read]);

  if (!adapter || !view) return null;

  const run = async (op: "repoCache.set" | "repoCache.clear", args: RepoCacheChange, what: string) => {
    if (busy) return;
    setBusy(true);
    try {
      setView(await adapter.call(op, args));
    } catch (e) {
      toastError(what)(e);
      setDraft(latest.current);
    } finally {
      setBusy(false);
    }
  };

  const saveSize = () => {
    const text = draft.trim();
    const n = text === "" ? null : Number(text);
    if (n !== null && (!Number.isFinite(n) || n < 0)) return setDraft(shown);
    if ((n === null ? "" : String(n)) === shown) return setDraft(shown);
    void run("repoCache.set", { sizeGb: n }, "Couldn't change the cache size");
  };

  return (
    <SettingsGroup title="Cache" data-settings-group="repo-cache">
      <SettingsRow
        label="Repo and dependency cache"
        htmlFor="repo-cache"
        description={
          <>
            Keeps copies of public repositories on this machine, so cloning one again in a Space is fast. Nothing private is kept.
            <span className="block" data-repo-cache-summary>
              {view.summary}
            </span>
            <span className="block" data-repo-cache-week>
              {view.week}
            </span>
          </>
        }
        control={
          <Switch
            id="repo-cache"
            data-setting-row="repo-cache"
            checked={view.enabled}
            disabled={busy}
            onCheckedChange={(on) => void run("repoCache.set", { enabled: on }, "Couldn't change the cache")}
          />
        }
      />
      {view.enabled ? (
        <>
          <SettingsRow
            label="Size limit"
            htmlFor="repo-cache-size"
            description="The least recently used repositories go first. Empty is automatic: the smaller of 5 GB and a tenth of the free disk."
            control={
              <span className="flex items-center gap-2">
                <Input
                  id="repo-cache-size"
                  data-setting-row="repo-cache-size"
                  type="number"
                  inputMode="decimal"
                  min={0}
                  className="w-28 text-right"
                  placeholder="Automatic"
                  value={draft}
                  disabled={busy}
                  onValueChange={setDraft}
                  onBlur={saveSize}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                  }}
                />
                <span className="w-12 text-[13px] text-muted-foreground">GB</span>
              </span>
            }
          />
          <SettingsRow
            label="Clear cache"
            description="Deletes the copies. They are made again the next time a Space clones them."
            control={
              <Button size="sm" variant="outline" data-setting-row="repo-cache-clear" disabled={busy || view.repos === 0} onClick={() => void run("repoCache.clear", {}, "Couldn't clear the cache")}>
                Clear cache
              </Button>
            }
          />
        </>
      ) : null}
    </SettingsGroup>
  );
}
