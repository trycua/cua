// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Link } from "@tanstack/react-router";
import { CheckIcon, ChevronRightIcon, MousePointer2Icon } from "lucide-react";
import { useState } from "react";

import { pointerChange, pointerSettings, POINTER_STYLES, useBridge, useDriver, useDriverMachines, type DriverApp, type DriverHook, type DriverPermission, type DriverStatus, type PointerSettings } from "@/bridge";
import { AppMarkImage } from "@/components/agents/harness-mark";
import { CuaKey } from "@/components/cua-key";
import { OsIcon } from "@/components/os-icon";
import { EmptyState, Page } from "@/components/page";
import { Button } from "@/components/ui/button";
import { GroupedSection, PlainList, plainRowClass, SettingRow } from "@/components/ui/grouped";
import { Segmented } from "@/components/ui/segmented";
import { Select } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { toast, toastError } from "@/components/ui/toast";
import { cn } from "@/lib/utils";
import { DRIVER_LINE, markFor, stateText, testText, versionText } from "./copy";
import { PointerPreview } from "./pointer-preview";

/**
 * The Cua Driver page (CUA-2706, variation A): what it is in one line, its
 * state, version and Update, Test now and Pause, a live preview of the agent
 * pointer, the pointer's settings, the coding apps that use it, the two
 * System Settings switches, and the account's other computers. A part the
 * host can't answer hides.
 */
export function DriverPage({ machine }: { machine: string | null }) {
  const driver = useDriver(machine);
  if (driver.loading) return <Page className="max-w-5xl">{null}</Page>;
  if (!driver.available || !driver.status) {
    return (
      <Page className="max-w-3xl">
        <EmptyState icon={<MousePointer2Icon />} title="Cua Driver isn't on this computer yet">
          It comes with an update to Cua Spaces.
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
      <Hero driver={driver} status={status} settings={settings} />
      <div className="mt-8 grid gap-x-6 [@media(min-width:900px)]:grid-cols-2">
        {hasPointer ? <PointerSection driver={driver} settings={settings} /> : null}
        <div className="min-w-0">
          {apps && apps.length > 0 ? <AppsSection driver={driver} apps={apps} /> : null}
          {status.permissions.length > 0 ? <PermissionsSection permissions={status.permissions} /> : null}
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
  const [busy, setBusy] = useState<"test" | "update" | "pause" | null>(null);
  const run = async (what: "test" | "update" | "pause", work: () => Promise<void>) => {
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
  const update = () =>
    run("update", async () => {
      const to = status.update?.version;
      await driver
        .update()
        .then(() => toast(`Cua Driver ${to} is installed`, { description: "It passed its test. No new permission was needed." }))
        .catch(toastError("Couldn't update Cua Driver"));
    });
  const pause = () => run("pause", () => driver.pause(status.state !== "paused").then(() => undefined).catch(toastError("Couldn't pause agents")));
  const ok = status.state === "ready";
  const tested = testText(status);
  const version = versionText(status);
  return (
    <section className="grid items-center gap-8 [@media(min-width:900px)]:grid-cols-[1fr_minmax(0,400px)]" data-driver-hero="">
      <div className="flex min-w-0 gap-5">
        <CuaKey size={56} className="mt-1 size-14 self-start" />
        <div className="min-w-0">
          <h1 className="font-display text-[28px] leading-tight font-semibold tracking-[-0.02em]">Cua Driver</h1>
          <p className="mt-1 text-[14px] text-muted-foreground">{status.thisMachine ? DRIVER_LINE : `Lets agents see and use the apps on ${status.machineName}.`}</p>
          <p className="mt-4 flex items-start gap-2 text-[13px]" data-driver-state={status.state}>
            {ok ? (
              <span className="mt-px flex size-4 shrink-0 items-center justify-center rounded-full bg-[#1f9d55] text-white" aria-hidden>
                <CheckIcon className="size-2.5" strokeWidth={3} />
              </span>
            ) : null}
            <span>
              <span className="font-medium">{stateText(status)}</span>
              {tested ? <span className="text-muted-foreground"> {busy === "test" ? "Testing now." : tested}</span> : busy === "test" ? <span className="text-muted-foreground"> Testing now.</span> : null}
            </span>
          </p>
          {status.state === "failed" && status.lastError ? <p className="mt-1 text-[13px] text-destructive">{status.lastError}</p> : null}
          {version ? (
            <p className="mt-1 text-[13px] text-muted-foreground" data-driver-version="">
              {version}
            </p>
          ) : null}
          <div className="mt-5 flex flex-wrap items-center gap-2">
            {status.update ? (
              <Button disabled={busy !== null} onClick={() => void update()} data-driver-update="">
                {busy === "update" ? "Updating…" : `Update to ${status.update.version}`}
              </Button>
            ) : null}
            <Button variant={status.update ? "outline" : "default"} disabled={busy !== null} onClick={() => void test()} data-driver-test="">
              {busy === "test" ? "Testing…" : "Test now"}
            </Button>
            {status.canPause ? (
              <Button variant="outline" disabled={busy !== null} onClick={() => void pause()} data-driver-pause="">
                {status.state === "paused" ? "Resume agents" : "Pause agents"}
              </Button>
            ) : null}
          </div>
          {status.update ? <p className="mt-2 text-xs text-muted-foreground">Takes about a minute. No new permission. If the test fails, it goes back to {status.version}.</p> : null}
        </div>
      </div>
      {status.thisMachine ? <PointerPreview settings={settings} caption="Live: the agent pointer as it looks on this Mac" /> : null}
    </section>
  );
}

/* ---- Agent pointer --------------------------------------------------------------- */

function PointerSection({ driver, settings }: { driver: DriverHook; settings: PointerSettings }) {
  const change = <K extends keyof PointerSettings>(key: K, value: NonNullable<PointerSettings[K]>) =>
    void driver.setConfig(pointerChange(key, value)).catch(toastError("Couldn't change the pointer"));
  const off = settings.show === false;
  return (
    <GroupedSection title="Agent pointer" className="mb-8 min-w-0" data-driver-pointer="" footer="Applies to every agent on this Mac.">
      {settings.show !== null ? (
        <SettingRow
          label="Show the agent's pointer"
          help="A second pointer shows where an agent is working. Yours never moves."
          control={<Switch checked={settings.show} onCheckedChange={(on) => change("show", on)} aria-label="Show the agent's pointer" />}
        />
      ) : null}
      {settings.style !== null ? (
        <SettingRow
          label="Style"
          help="How it travels between clicks."
          dim={off}
          control={<Select aria-label="Pointer style" value={settings.style} options={POINTER_STYLES.some((s) => s.value === settings.style) ? POINTER_STYLES : [...POINTER_STYLES, { value: settings.style, label: settings.style }]} onValueChange={(v) => change("style", v)} disabled={off} />}
        />
      ) : null}
      {settings.float !== null ? (
        <SettingRow label="Float while it thinks" help="A slow bob while the agent pauses." dim={off} control={<Switch checked={settings.float} disabled={off} onCheckedChange={(on) => change("float", on)} aria-label="Float while it thinks" />} />
      ) : null}
      {settings.marks !== null ? (
        <SettingRow label="Show what it's doing" help="A small sign next to it: moving, clicking, typing." dim={off} control={<Switch checked={settings.marks} disabled={off} onCheckedChange={(on) => change("marks", on)} aria-label="Show what it's doing" />} />
      ) : null}
      {settings.rotate !== null ? (
        <SettingRow label="Tilt while moving" help="The arrow leans the way it's going." dim={off} control={<Switch checked={settings.rotate} disabled={off} onCheckedChange={(on) => change("rotate", on)} aria-label="Tilt while moving" />} />
      ) : null}
      {settings.nameTag !== null ? (
        <SettingRow
          label="Name tag"
          help="Which app is moving it."
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

function AppsSection({ driver, apps }: { driver: DriverHook; apps: DriverApp[] }) {
  return (
    <GroupedSection title="Coding apps" className="mb-8" data-driver-apps="">
      {apps.map((a) => (
        <SettingRow
          key={a.id}
          data-driver-app={a.id}
          label={
            <span className="flex items-center gap-2.5">
              <AppMarkImage agent={markFor(a.id)} className="size-4" />
              {a.name}
            </span>
          }
          help={a.bringsOwn ? "Brings its own Cua Driver, with its own permission." : a.own ? "Uses your own Cua Driver setup. Cua Spaces leaves it as it is." : a.on ? "Uses Cua Driver" : "Doesn't use Cua Driver"}
          control={a.bringsOwn || a.own ? undefined : <Switch checked={a.on} onCheckedChange={(on) => void driver.setApp(a.id, on).catch(toastError(`Couldn't change ${a.name}`))} aria-label={`${a.name} uses Cua Driver`} />}
        />
      ))}
    </GroupedSection>
  );
}

const PERMISSION_WORDS: Record<DriverPermission["id"], { label: string; help: string }> = {
  screen: { label: "See the screen", help: "Screen Recording, given to Cua Spaces." },
  accessibility: { label: "Use the mouse and keyboard", help: "Accessibility, given to Cua Spaces." },
};

function PermissionsSection({ permissions }: { permissions: DriverPermission[] }) {
  const { data: adapter } = useBridge();
  return (
    <GroupedSection title="Permissions" className="mb-8" data-driver-permissions="" footer="Given to Cua Spaces once. Cua Driver updates keep it.">
      {permissions.map((p) => (
        <SettingRow
          key={p.id}
          data-driver-permission={p.id}
          label={PERMISSION_WORDS[p.id].label}
          help={PERMISSION_WORDS[p.id].help}
          value={p.granted ? "On" : undefined}
          control={
            p.granted || !p.settingsUrl ? undefined : (
              <Button variant="outline" size="sm" onClick={() => void adapter?.call("host.openSettings", { url: p.settingsUrl! }).catch(toastError("Couldn't open System Settings"))}>
                Allow
              </Button>
            )
          }
        />
      ))}
    </GroupedSection>
  );
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
                to="/driver"
                search={m.thisMachine ? {} : { machine: m.machineId }}
                className={cn(plainRowClass(m.machineId === current))}
                data-driver-computer={m.machineId}
              >
                <OsIcon os={m.os} className="size-4 text-muted-foreground" />
                <span className="flex-1 truncate">{m.thisMachine ? "This Mac" : m.name}</span>
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
