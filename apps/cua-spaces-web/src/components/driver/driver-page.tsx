// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Link } from "@tanstack/react-router";
import { CheckIcon, ChevronRightIcon, MousePointer2Icon } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import {
  pointerChange,
  pointerSettings,
  POINTER_STYLES,
  useBridge,
  useDriver,
  useDriverMachines,
  type DriverApp,
  type DriverHook,
  type DriverStatus,
  type PointerSettings,
} from "@/bridge";
import { useAppPermissions } from "@/bridge/app-permissions";
import { AppMarkImage } from "@/components/agents/harness-mark";
import { AppPermissionList } from "@/components/app-permissions";
import { BackLink } from "@/components/back-link";
import { CuaKey } from "@/components/cua-key";
import { OsIcon } from "@/components/os-icon";
import { EmptyState, Page } from "@/components/page";
import { Button } from "@/components/ui/button";
import { GroupedSection, PlainList, plainRowClass, SettingRow } from "@/components/ui/grouped";
import { Segmented } from "@/components/ui/segmented";
import { Select } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { toast, toastError } from "@/components/ui/toast";
import { repairOutcome } from "@/lib/app-permissions";
import { thisComputerLabel, thisComputerText } from "@/lib/host-labels";
import { THIS_COMPUTER } from "@/lib/machines";
import { cn } from "@/lib/utils";
import { driverLine, markFor, stateText, testText, versionText } from "./copy";
import { PointerPreview } from "./pointer-preview";

/**
 * The Cua Driver page (CUA-2706, design A): what it is in one line, its
 * state and version, Set up or Repair and Test now, the switch that lets
 * agents use this Mac, a live preview of the agent pointer and its
 * settings, the coding agents connected to it, and the two permissions it
 * uses (Cua Spaces' own). A part the host can't answer hides.
 */
export function DriverPage({ machine }: { machine: string | null }) {
  const driver = useDriver(machine);
  if (driver.loading) return <Page className="max-w-5xl">{null}</Page>;
  if (!driver.available || !driver.status) {
    return (
      <Page className="max-w-3xl">
        {machine ? <BackLink /> : <BackLink computerId={THIS_COMPUTER} label={thisComputerLabel()} />}
        <EmptyState icon={<MousePointer2Icon />} title="Cua Driver isn't on this computer yet">
          It comes with Cua Spaces for Mac.
        </EmptyState>
      </Page>
    );
  }
  return <DriverBody driver={driver} status={driver.status} />;
}

function DriverBody({ driver, status }: { driver: DriverHook; status: DriverStatus }) {
  const settings = pointerSettings(driver.config);
  const hasPointer = status.thisMachine && Object.values(settings).some((v) => v !== null);
  const apps = status.thisMachine ? status.apps : null;
  return (
    <Page className="max-w-5xl">
      <div data-driver-page={status.machineId} />
      <BackLink computerId={status.thisMachine ? THIS_COMPUTER : status.machineId} label={status.thisMachine ? thisComputerLabel() : status.machineName} />
      <Hero driver={driver} status={status} settings={settings} />
      {status.thisMachine && status.canPause ? <AllowSection driver={driver} status={status} /> : null}
      <div className="grid gap-x-6 [@media(min-width:900px)]:grid-cols-2">
        {hasPointer ? <PointerSection driver={driver} settings={settings} /> : null}
        <div className="min-w-0">
          {apps ? <AppsSection apps={apps} /> : null}
          {status.thisMachine ? <PermissionsSection driver={driver} /> : null}
          {!status.thisMachine ? (
            <GroupedSection title="On that computer" className="mb-8">
              <SettingRow label="Pointer and coding apps" help={`Open Cua Spaces on ${status.machineName} to change them.`} />
            </GroupedSection>
          ) : null}
        </div>
      </div>
      <Computers current={status.machineId} />
    </Page>
  );
}

/* ---- The top: keycap, the line, state, version, actions, preview ---------------- */

function Hero({ driver, status, settings }: { driver: DriverHook; status: DriverStatus; settings: PointerSettings }) {
  const bridge = useBridge();
  const [busy, setBusy] = useState<"test" | "setup" | null>(null);
  const run = async (what: "test" | "setup", work: () => Promise<void>) => {
    setBusy(what);
    try {
      await work();
    } finally {
      setBusy(null);
    }
  };
  const test = () =>
    run("test", async () => {
      const t = await driver.test().catch((e) => {
        toastError("Couldn't run the test")(e);
        return null;
      });
      if (t && !t.passed) toast("The test didn't pass", { description: t.failure ?? undefined, type: "error" });
    });
  const connected = (status.apps ?? []).some((a) => a.on);
  const setup = () =>
    run("setup", async () => {
      const after = await driver.setup().catch((e) => {
        toastError(connected ? "Couldn't repair Cua Driver" : "Couldn't set up Cua Driver")(e);
        return undefined;
      });
      if (after === undefined) return;
      if (!after) return;
      const outcome = repairOutcome(after);
      if (outcome.kind === "ok") toast(outcome.title, { description: outcome.description });
      else toast(outcome.title, { description: outcome.description, type: "error" });
      // A switch that's off: open its guide now, so Repair leaves one thing to do.
      if (outcome.kind === "permission" && outcome.settingsUrl)
        await bridge.data?.call("host.openSettings", { url: outcome.settingsUrl }).catch(toastError("Couldn't open System Settings"));
    });
  const ok = status.state === "ready";
  const tested = testText(status);
  const version = versionText(status);
  const problem = (status.state === "failed" || status.state === "stopped") && status.lastError ? status.lastError : null;
  return (
    <section className="grid items-center gap-8 [@media(min-width:900px)]:grid-cols-[1fr_minmax(0,400px)]" data-driver-hero="">
      <div className="flex min-w-0 gap-5">
        <CuaKey size={56} className="mt-1 size-14 self-start" />
        <div className="min-w-0">
          <h1 className="font-display text-[28px] leading-tight font-semibold tracking-[-0.02em]">Cua Driver</h1>
          <p className="mt-1 text-[14px] text-muted-foreground">
            {status.thisMachine ? driverLine() : `Lets agents see and use the apps on ${status.machineName}.`}
          </p>
          <p className="mt-4 flex items-start gap-2 text-[13px]" data-driver-state={status.state}>
            {ok ? (
              <span className="mt-px flex size-4 shrink-0 items-center justify-center rounded-full bg-[#1f9d55] text-white" aria-hidden>
                <CheckIcon className="size-2.5" strokeWidth={3} />
              </span>
            ) : null}
            <span>
              <span className="font-medium">{stateText(status)}</span>
              {busy === "test" ? (
                <span className="text-muted-foreground"> Testing now.</span>
              ) : tested ? (
                <span className="text-muted-foreground"> {tested}</span>
              ) : null}
            </span>
          </p>
          {problem ? (
            <p className="mt-1 text-[13px] text-muted-foreground" data-driver-problem="">
              {problem}
            </p>
          ) : null}
          {version ? (
            <p className="mt-1 text-[13px] text-muted-foreground" data-driver-version="">
              {version}
            </p>
          ) : null}
          {status.thisMachine ? (
            <div className="mt-5 flex flex-wrap items-center gap-2">
              <Button
                disabled={busy !== null}
                onClick={() => void setup()}
                data-driver-setup={connected ? "repair" : "setup"}
                title={
                  connected
                    ? "Connects your coding agents again, restarts Cua Driver and tests it."
                    : "Connects Claude Code and Codex to Cua Driver and tests it."
                }
              >
                {busy === "setup" ? (connected ? "Repairing…" : "Setting up…") : connected ? "Repair" : "Set up"}
              </Button>
              <Button variant="outline" disabled={busy !== null || status.state === "paused"} onClick={() => void test()} data-driver-test="">
                {busy === "test" ? "Testing…" : "Test now"}
              </Button>
            </div>
          ) : null}
        </div>
      </div>
      {status.thisMachine ? <PointerPreview settings={settings} /> : null}
    </section>
  );
}

/* ---- Let agents use this computer ---------------------------------------------------- */

function AllowSection({ driver, status }: { driver: DriverHook; status: DriverStatus }) {
  const on = status.state !== "paused";
  return (
    <GroupedSection title="Agents" className="mt-8 mb-8" data-driver-allow="">
      <SettingRow
        label={`Let agents use apps on ${thisComputerText()}`}
        help={
          on
            ? "Coding agents you connect can see your screen and use your apps. Your own pointer never moves."
            : "Off. Agents can't see your screen or use your apps until you turn this on."
        }
        control={
          <Switch
            checked={on}
            onCheckedChange={(next) => void driver.pause(!next).catch(toastError(next ? "Couldn't turn agents on" : "Couldn't turn agents off"))}
            aria-label={`Let agents use apps on ${thisComputerText()}`}
          />
        }
      />
    </GroupedSection>
  );
}

/* ---- Agent pointer --------------------------------------------------------------- */

function PointerSection({ driver, settings }: { driver: DriverHook; settings: PointerSettings }) {
  const change = <K extends keyof PointerSettings>(key: K, value: NonNullable<PointerSettings[K]>) =>
    void driver.setConfig(pointerChange(key, value)).catch(toastError("Couldn't change the pointer"));
  const off = settings.show === false;
  return (
    <GroupedSection title="Agent pointer" className="mb-8 min-w-0" data-driver-pointer="">
      {settings.show !== null ? (
        <SettingRow
          label="Show the agent's pointer"
          help="Shows where an agent is working."
          control={<Switch checked={settings.show} onCheckedChange={(on) => change("show", on)} aria-label="Show the agent's pointer" />}
        />
      ) : null}
      {settings.style !== null ? (
        <SettingRow
          label="Style"
          dim={off}
          control={
            <Select
              aria-label="Pointer style"
              value={settings.style}
              options={
                POINTER_STYLES.some((s) => s.value === settings.style) ? POINTER_STYLES : [...POINTER_STYLES, { value: settings.style, label: settings.style }]
              }
              onValueChange={(v) => change("style", v)}
              disabled={off}
            />
          }
        />
      ) : null}
      {settings.float !== null ? (
        <SettingRow
          label="Float while it thinks"
          dim={off}
          control={<Switch checked={settings.float} disabled={off} onCheckedChange={(on) => change("float", on)} aria-label="Float while it thinks" />}
        />
      ) : null}
      {settings.marks !== null ? (
        <SettingRow
          label="Show what it's doing"
          dim={off}
          control={<Switch checked={settings.marks} disabled={off} onCheckedChange={(on) => change("marks", on)} aria-label="Show what it's doing" />}
        />
      ) : null}
      {settings.rotate !== null ? (
        <SettingRow
          label="Tilt while moving"
          dim={off}
          control={<Switch checked={settings.rotate} disabled={off} onCheckedChange={(on) => change("rotate", on)} aria-label="Tilt while moving" />}
        />
      ) : null}
      {settings.nameTag !== null ? (
        <SettingRow
          label="Name tag"
          dim={off}
          control={
            <Segmented
              aria-label="Name tag"
              value={settings.nameTag}
              disabled={off}
              options={[
                { value: "minimal", label: "When needed" },
                { value: "verbose", label: "Always" },
                { value: "off", label: "Never" },
              ]}
              onValueChange={(v) => change("nameTag", v)}
            />
          }
        />
      ) : null}
    </GroupedSection>
  );
}

/* ---- Coding apps and permissions --------------------------------------------- */

function appLine(a: DriverApp): string {
  if (a.bringsOwn) return "Brings its own Cua Driver, with its own permission.";
  if (a.on) return "Uses Cua Driver in Cua Spaces.";
  if (a.own) return "Uses another Cua Driver. Set up switches it to this one.";
  return "Not connected. Set up connects it.";
}

function AppsSection({ apps }: { apps: DriverApp[] }) {
  return (
    <GroupedSection title="Connected agents" className="mb-8" data-driver-apps="">
      {apps.length === 0 ? <SettingRow label="No coding agents yet" help="Install Claude Code or Codex, then choose Set up." /> : null}
      {apps.map((a) => (
        <SettingRow
          key={a.id}
          data-driver-app={a.id}
          data-driver-app-on={a.on ? "" : undefined}
          label={
            <span className="flex items-center gap-2.5">
              <AppMarkImage agent={markFor(a.id)} className="size-4" />
              {a.name}
            </span>
          }
          help={appLine(a)}
          value={a.bringsOwn ? undefined : a.on ? "Connected" : undefined}
        />
      ))}
    </GroupedSection>
  );
}

/** Cua Spaces' own two permissions, live (the first run's step shows the same rows); when both come on, the driver's state is read again. */
function PermissionsSection({ driver }: { driver: DriverHook }) {
  const perms = useAppPermissions();
  const { allGranted } = perms;
  const refresh = useRef(driver.refresh);
  refresh.current = driver.refresh;
  useEffect(() => {
    if (allGranted) void refresh.current().catch(() => {});
  }, [allGranted]);
  if (!perms.data) return null;
  return <AppPermissionList perms={perms} variant="settings" />;
}

/* ---- The account's other computers -------------------------------------------- */

function Computers({ current }: { current: string }) {
  const machines = useDriverMachines(true);
  if (machines.length < 2) return null;
  return (
    <section data-driver-computers="">
      <GroupedSection title="Your computers" className="mb-8">
        <PlainList className="p-1">
          {machines.map((m) => {
            const line = m.update ? `Cua Driver ${m.version ?? ""}, ${m.update} ready` : m.version ? `Cua Driver ${m.version}` : "No Cua Driver yet";
            return (
              <Link
                key={m.machineId}
                to="/spaces/computer/$computerId/driver"
                params={{ computerId: m.thisMachine ? THIS_COMPUTER : m.machineId }}
                className={cn(plainRowClass(m.machineId === current))}
                data-driver-computer={m.machineId}
              >
                <OsIcon os={m.os} className="size-4 text-muted-foreground" />
                <span className="flex-1 truncate">{m.thisMachine ? thisComputerLabel() : m.name}</span>
                <span className="truncate text-xs text-muted-foreground">{line}</span>
                <ChevronRightIcon className="size-3.5 text-muted-foreground" />
              </Link>
            );
          })}
        </PlainList>
      </GroupedSection>
    </section>
  );
}
