// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useState } from "react";

import { dataWords, useExperiments, useShareSettings, type ShareLanding } from "@/bridge";
import { allowProblem, parseAllow } from "@/components/links/tcp-words";
import { SettingsGroup, SettingsRow } from "@/components/settings-group";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/alert-dialog";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { toastError } from "@/components/ui/toast";

const LANDING: { value: ShareLanding; label: string }[] = [
  { value: "auto", label: "Automatic" },
  { value: "only_my_cloud", label: "Only my cloud" },
  { value: "allow_relay", label: "Allow Cua relay" },
];

const LANDING_HELP: Record<ShareLanding, string> = {
  auto: "The Space's own computer in your cloud, then a small server in your cloud, then Cua.",
  only_my_cloud: "Shares never go through Cua. Without a cloud of yours, a share waits.",
  allow_relay: "Cua carries a share unless the Space already runs in your cloud.",
};

/** MiB a day: Off, 1, 2, 5 or 10 GB. */
const CAPS = [
  { value: 0, label: "Off" },
  { value: 1024, label: "1 GB" },
  { value: 2048, label: "2 GB" },
  { value: 5120, label: "5 GB" },
  { value: 10240, label: "10 GB" },
];

const WHO = [
  { value: "anyone", label: "Anyone" },
  { value: "only", label: "Only these addresses" },
];

/** "1.4 GB of 2 GB used today" or, with no cap, "1.4 GB used today". */
export function usedWords(usedBytes: number, capMib: number): string {
  const cap = CAPS.find((c) => c.value === capMib)?.label ?? `${capMib} MB`;
  return capMib === 0 ? `${dataWords(usedBytes)} used today` : `${dataWords(usedBytes)} of ${cap} used today`;
}

/**
 * Settings, Sharing (Labs: Public TCP shares): where shares go, the daily data
 * cap, who may connect by default, whether an agent's share asks first, and
 * the share server in your cloud when there is one.
 */
export function SharingSettings() {
  const on = useExperiments().data?.experiments.tcpShares === true;
  const { settings, unsupported, save, removeEdge } = useShareSettings({ load: on });
  const [addresses, setAddresses] = useState("");
  const [only, setOnly] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const saved = settings?.defaultAllow.join(", ") ?? "";
  useEffect(() => {
    setAddresses(saved);
    if (saved !== "") setOnly(true);
  }, [saved]);
  if (!on || unsupported || !settings) return null;

  const change = (patch: Parameters<typeof save>[0]) => void save(patch).catch(toastError("Could not change the setting"));
  const list = parseAllow(addresses);
  const problem = list.length === 0 ? null : allowProblem(list);

  return (
    <SettingsGroup title="Sharing" data-sharing-settings="">
      <SettingsRow
        label="Where shares go"
        description={LANDING_HELP[settings.landing]}
        control={
          <Select
            aria-label="Where shares go"
            value={settings.landing}
            options={LANDING}
            onValueChange={(landing) => change({ landing, relayFallback: landing !== "only_my_cloud" })}
          />
        }
      />
      <SettingsRow
        label="Daily data cap"
        description={<span data-sharing-used="">{usedWords(settings.usedToday, settings.dailyCapMib)}</span>}
        control={<Select aria-label="Daily data cap" value={settings.dailyCapMib} options={CAPS} onValueChange={(dailyCapMib) => change({ dailyCapMib })} />}
      />
      <SettingsRow
        label="Who can connect by default"
        description={problem ?? (only ? "Separate addresses with commas." : undefined)}
        control={
          <div className="flex items-center gap-2">
            {only ? (
              <Input
                aria-label="Addresses"
                placeholder="203.0.113.0/24"
                className="w-48"
                value={addresses}
                onChange={(e) => setAddresses(e.target.value)}
                onBlur={() => !problem && change({ defaultAllow: list })}
                data-sharing-allow=""
              />
            ) : null}
            <Select
              aria-label="Who can connect by default"
              className="min-w-40"
              value={only ? "only" : "anyone"}
              options={WHO}
              onValueChange={(v) => {
                setOnly(v === "only");
                if (v === "anyone") {
                  setAddresses("");
                  change({ defaultAllow: [] });
                }
              }}
            />
          </div>
        }
      />
      <SettingsRow
        label="Ask before an agent shares"
        description="An agent in a Space asks you before its share goes public."
        control={
          <Switch
            aria-label="Ask before an agent shares"
            checked={settings.approveAgentShares}
            onCheckedChange={(approveAgentShares) => change({ approveAgentShares })}
            data-sharing-ask=""
          />
        }
      />
      {settings.edgeHost ? (
        <SettingsRow
          label="Share server"
          description={
            <span data-sharing-edge="">
              {settings.edgeHost} in {settings.edgeCloud ?? "your cloud"}
              {settings.edgeMonthlyUsd != null ? `, about $${Math.round(settings.edgeMonthlyUsd)} a month` : ""}
            </span>
          }
          control={
            <Button size="sm" variant="outline" onClick={() => setConfirm(true)} data-sharing-edge-remove="">
              Remove
            </Button>
          }
        />
      ) : null}
      <ConfirmDialog
        open={confirm}
        onOpenChange={setConfirm}
        title="Remove the share server?"
        description="Shares through it stop now. The next one that needs a server asks you again."
        confirmLabel="Remove"
        destructive
        onConfirm={() => {
          setConfirm(false);
          void removeEdge().catch(toastError("Could not remove the share server"));
        }}
      />
    </SettingsGroup>
  );
}
