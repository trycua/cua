// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute } from "@tanstack/react-router";
import { CircleHelpIcon, EllipsisIcon, HashIcon, LaptopIcon, MonitorIcon, SmartphoneIcon, UserRoundIcon, UsersIcon } from "lucide-react";
import { useState } from "react";

import { useDevices, useSession, type DeviceRow, type DevicesLabels, type UnconfirmedMachine } from "@/bridge";
import { ConfirmDialog } from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Dialog, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Menu } from "@/components/ui/menu";
import { deviceKind, devicesPageState, relativeText, rowDetail, thisDeviceText } from "@/lib/devices";
import { thisComputerLabel } from "@/lib/host-labels";
import { cn } from "@/lib/utils";

export const Route = createFileRoute("/settings/devices")({ component: DevicesPage });

/** `devices::labels().signed_out`, for before the core's view exists. */
const SIGNED_OUT = "Sign in to Cua to see the devices that can reach your machines.";

/**
 * Settings, Devices: this device's enrollment, new machines to confirm, the
 * account's devices (Approve, Deny, Rename, Revoke) and Recent Access, then
 * the enroll and approval sheets. Everything shown is the app core's
 * `devices::devices_view`, as in the SwiftUI app's `DevicesSettingsView`.
 */
function DevicesPage() {
  const { data: session } = useSession();
  const devices = useDevices();
  const [renaming, setRenaming] = useState<DeviceRow | null>(null);
  const [revoking, setRevoking] = useState<DeviceRow | null>(null);
  const [confirming, setConfirming] = useState<UnconfirmedMachine | null>(null);

  const state = devicesPageState(devices, session?.signedIn);
  if (state === "unsupported") return <Muted>Devices are not available in this app yet.</Muted>;
  if (state === "signed-out") return <Muted>{devices.data?.view?.labels.signedOut ?? SIGNED_OUT}</Muted>;
  if (state === "error" || state === "no-view") {
    return (
      <Retry onRetry={() => void devices.refresh()} data-devices-error>
        {devices.error ? `Couldn't read your devices: ${devices.error.message}` : "Couldn't show your devices."}
      </Retry>
    );
  }
  // The host may take a while to ask the relay.
  if (state === "loading") return <Muted data-devices-loading>Loading devices…</Muted>;
  const data = devices.data!;
  const v = data.view!;
  const labels = v.labels;

  return (
    <div data-devices>
      <Group title={labels.thisDevice}>
        <div className="flex min-h-12 items-center justify-between gap-4 px-4 py-2.5">
          <div className="flex min-w-0 items-center gap-2.5">
            <LaptopIcon className="size-4 shrink-0 text-muted-foreground" strokeWidth={1.75} />
            {/* Named as the system names it ("cua's Mac Studio"), as the SwiftUI app does. */}
            <span className="truncate text-[13px]" data-this-device-name>
              {v.thisDevice.name ?? data.input.deviceName ?? thisComputerLabel()}
            </span>
          </div>
          <div className="flex shrink-0 items-center gap-2">
            <span data-this-device-status={v.thisDevice.kind} className={cn("text-[13px]", v.thisDevice.kind === "enrolled" ? "text-muted-foreground" : "text-warning")}>
              {thisDeviceText(v.thisDevice)}
            </span>
            {v.thisDevice.actionLabel ? (
              <Button size="sm" variant="outline" onClick={devices.startEnroll} data-this-device-enroll>
                {v.thisDevice.actionLabel}
              </Button>
            ) : null}
          </div>
        </div>
        {v.banner ? (
          // What it means, under the row (its one Enroll… is the row's), as
          // the SwiftUI app's Devices settings have it.
          <p data-devices-banner={v.banner.tone} className="px-4 py-2.5 text-xs text-muted-foreground">
            <span data-banner-text>{v.banner.text}</span>
          </p>
        ) : null}
      </Group>

      {v.unconfirmedMachines.length > 0 ? (
        <Group title={labels.newMachines}>
          {v.unconfirmedMachines.map((m) => (
            <div key={m.id} data-unconfirmed-machine={m.id} className="flex min-h-11 items-center gap-2.5 px-4 py-2">
              <CircleHelpIcon className="size-4 shrink-0 text-warning" />
              <span className="min-w-0 flex-1 truncate text-[13px]">{m.title}</span>
              <Button size="sm" variant="outline" onClick={() => setConfirming(m)}>
                {labels.confirmMachine}
              </Button>
            </div>
          ))}
        </Group>
      ) : null}

      {v.rows.length > 0 ? (
        <Group title={labels.devices}>
          {v.rows.map((r) => (
            <DeviceLine
              key={r.id}
              row={r}
              labels={labels}
              now={data.now}
              busy={data.busy}
              onApprove={() => devices.openApproval(r.id)}
              onDeny={() => void devices.denyDevice(r.id)}
              onRename={() => setRenaming(r)}
              onRevoke={() => setRevoking(r)}
            />
          ))}
        </Group>
      ) : null}

      <Group title={labels.recent} data-recent>
        {v.recent.length === 0 ? <div className="px-4 py-3 text-[13px] text-muted-foreground">{labels.recentEmpty}</div> : null}
        {v.recent.map((a, i) => (
          <div key={`${a.ts}-${i}`} data-recent-row className="flex min-h-10 items-center justify-between gap-4 px-4 py-2">
            <span className="flex min-w-0 items-center gap-2 text-[13px]">
              {a.notable ? (
                <span title="Another account or an unenrolled device">
                  <UsersIcon className="size-3.5 shrink-0 text-warning" aria-label="Another account or an unenrolled device" />
                </span>
              ) : null}
              <span className="truncate" data-recent-text>
                {a.text}
              </span>
            </span>
            <span className="shrink-0 text-xs text-muted-foreground">{relativeText(a.ts, data.now)}</span>
          </div>
        ))}
      </Group>
      {/* The relay's own words only when the page has not already said it:
          "Needs enrollment" is the not-enrolled refusal. */}
      {data.input.readError && v.thisDevice.kind === "enrolled" ? (
        // The relay refused to list them (this device is not enrolled):
        // its words, under the page, as the SwiftUI app shows them.
        <p data-devices-read-error className="-mt-4 mb-7 px-1 text-xs break-words text-destructive">
          {data.input.readError}
        </p>
      ) : null}
      {data.actionError ? <p className="-mt-4 px-1 text-xs text-destructive">{data.actionError}</p> : null}

      <EnrollSheet />
      <ApproveSheet />
      <RenameDialog row={renaming} labels={labels} onClose={() => setRenaming(null)} onRename={(name) => renaming && void devices.rename(renaming.id, name)} />
      <ConfirmDialog
        open={revoking?.revokeConfirm != null}
        onOpenChange={(o) => !o && setRevoking(null)}
        title={revoking?.revokeConfirm?.title ?? ""}
        description={revoking?.revokeConfirm?.message ?? ""}
        confirmLabel={revoking?.revokeConfirm?.confirmLabel ?? labels.revoke}
        destructive
        onConfirm={() => revoking && void devices.revoke(revoking.id)}
      />
      <ConfirmDialog
        open={confirming !== null}
        onOpenChange={(o) => !o && setConfirming(null)}
        title={confirming?.confirm.title ?? ""}
        description={confirming?.confirm.message ?? ""}
        confirmLabel={confirming?.confirm.confirmLabel ?? labels.confirmMachine}
        onConfirm={() => confirming && void devices.confirmMachine(confirming.id)}
      />
    </div>
  );
}

function Muted({ children, ...rest }: { children: React.ReactNode } & Record<`data-${string}`, unknown>) {
  return (
    <p className="px-1 text-[13px] text-muted-foreground" {...rest}>
      {children}
    </p>
  );
}

function Retry({ children, onRetry, ...rest }: { children: React.ReactNode; onRetry: () => void } & Record<`data-${string}`, unknown>) {
  return (
    <div className="flex items-center gap-3 px-1" {...rest}>
      <p role="alert" className="min-w-0 flex-1 text-[13px] text-muted-foreground">
        {children}
      </p>
      <Button size="sm" variant="outline" onClick={onRetry}>
        Try again
      </Button>
    </div>
  );
}

function Group({ title, children, ...rest }: { title: string; children: React.ReactNode } & Record<`data-${string}`, unknown>) {
  return (
    <section className="mb-7" {...rest}>
      <h2 className="mb-2 px-1 text-xs font-semibold text-muted-foreground">{title}</h2>
      <div className="divide-y overflow-hidden rounded-xl border bg-card shadow-xs">{children}</div>
    </section>
  );
}

const ICONS = { laptop: LaptopIcon, desktop: MonitorIcon, phone: SmartphoneIcon, unknown: CircleHelpIcon } as const;

function DeviceLine({
  row: r,
  labels,
  now,
  busy,
  onApprove,
  onDeny,
  onRename,
  onRevoke,
}: {
  row: DeviceRow;
  labels: DevicesLabels;
  now: number;
  busy: boolean;
  onApprove: () => void;
  onDeny: () => void;
  onRename: () => void;
  onRevoke: () => void;
}) {
  const Icon = ICONS[deviceKind(r.platform)];
  const menu = [
    ...(r.actions.includes("rename") ? [{ label: labels.rename, onSelect: onRename }] : []),
    ...(r.actions.includes("revoke") ? [{ label: labels.revoke, onSelect: onRevoke, destructive: true }] : []),
  ];
  return (
    <div data-device-row={r.id} className="flex min-h-14 items-center gap-3 px-4 py-2.5">
      <Icon className="size-4 shrink-0 text-muted-foreground" strokeWidth={1.75} />
      <div className="min-w-0 flex-1">
        <div className="truncate text-[13px]" data-device-title>
          {r.title}
        </div>
        <div className="truncate text-xs text-muted-foreground" data-device-detail>
          {rowDetail(r, labels, now)}
        </div>
      </div>
      {r.actions.includes("approve") ? (
        <>
          <Button size="sm" variant="outline" onClick={onApprove} data-device-approve>
            {labels.approve}
          </Button>
          <Button size="sm" variant="ghost" disabled={busy} onClick={onDeny} data-device-deny>
            {labels.deny}
          </Button>
        </>
      ) : null}
      {menu.length > 0 ? (
        <Menu
          items={menu.map((m) => ({ ...m, disabled: busy }))}
          trigger={
            <Button size="icon-sm" variant="ghost" aria-label={`More for ${r.name || r.id}`} data-device-menu>
              <EllipsisIcon className="size-4 text-muted-foreground" />
            </Button>
          }
        />
      ) : null}
    </div>
  );
}

/** "Enroll This Device": sign in again, or approve from another device with a one-time code. */
function EnrollSheet() {
  const { data, chooseEnroll, backEnroll, closeEnroll } = useDevices();
  const v = data?.enroll?.view;
  return (
    <Dialog open={Boolean(v)} onOpenChange={(o) => !o && closeEnroll()}>
      <DialogPopup className="w-[min(440px,calc(100vw-2rem))]" data-enroll>
        {v ? (
          <>
            <div className="px-5 pt-5 pb-4">
              <DialogTitle className="text-[15px] font-semibold" data-enroll-title>
                {v.title}
              </DialogTitle>
              <p className="mt-1.5 text-[13px] text-muted-foreground">{v.lede}</p>
              {v.options.length > 0 ? (
                <div className="mt-4 flex flex-col gap-2">
                  {v.options.map((o) => (
                    <button
                      key={o.method}
                      type="button"
                      data-enroll-option={o.method}
                      onClick={() => void chooseEnroll(o.method)}
                      className="flex items-center gap-3 rounded-lg border bg-card px-3 py-2.5 text-left outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring/60"
                    >
                      {o.method === "sign-in" ? <UserRoundIcon className="size-5 text-muted-foreground" /> : <HashIcon className="size-5 text-muted-foreground" />}
                      <span className="min-w-0">
                        <span className="block text-[13px] font-medium">{o.title}</span>
                        <span className="block text-xs text-muted-foreground">{o.detail}</span>
                      </span>
                    </button>
                  ))}
                </div>
              ) : null}
              {v.code ? (
                <p className="mt-5 text-center font-mono text-[28px] font-semibold tracking-wider select-text" data-enroll-code>
                  {v.code}
                </p>
              ) : null}
              {v.codeHelp ? <p className="mt-3 text-xs text-muted-foreground">{v.codeHelp.replace(/`/g, "")}</p> : null}
              {v.status ? (
                <p className="mt-4 flex items-center gap-2 text-[13px] text-muted-foreground" data-enroll-status>
                  {v.busy ? <span className="size-3 animate-spin rounded-full border-2 border-muted-foreground/30 border-t-muted-foreground" /> : null}
                  {v.status}
                </p>
              ) : null}
              {v.error ? (
                <p className="mt-3 text-[13px] text-destructive" data-enroll-error>
                  {v.error}
                </p>
              ) : null}
            </div>
            <div className="flex justify-end gap-2 border-t bg-muted/60 px-5 py-3">
              {v.backLabel ? (
                <Button variant="outline" onClick={backEnroll} data-enroll-back>
                  {v.backLabel}
                </Button>
              ) : null}
              <Button variant={v.done ? "default" : "outline"} onClick={closeEnroll} data-enroll-close>
                {v.closeLabel}
              </Button>
            </div>
          </>
        ) : null}
      </DialogPopup>
    </Dialog>
  );
}

/** A device asking to join or to be re-verified: its code, Approve (presence first) and Deny or Not Now. */
function ApproveSheet() {
  const { data, setApprovalCode, approve, deny, closeApproval } = useDevices();
  const v = data?.approve?.view;
  return (
    <Dialog open={Boolean(v)} onOpenChange={(o) => !o && closeApproval()}>
      <DialogPopup className="w-[min(420px,calc(100vw-2rem))]" data-approve>
        {v ? (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              if (v.canApprove) void approve();
            }}
          >
            <div className="px-5 pt-5 pb-4">
              <DialogTitle className="text-[15px] font-semibold" data-approve-title>
                {v.title}
              </DialogTitle>
              <p className="mt-1.5 text-[13px] text-muted-foreground" data-approve-message>
                {v.message}
              </p>
              {v.needsCode ? (
                <Input
                  aria-label={v.codeLabel}
                  className="mt-4 h-10 font-mono text-[18px] tracking-wider"
                  placeholder={v.codePlaceholder}
                  value={v.code}
                  disabled={v.busy}
                  autoFocus
                  autoComplete="off"
                  spellCheck={false}
                  onChange={(e) => setApprovalCode(e.currentTarget.value)}
                  data-approve-code
                />
              ) : null}
              {v.error ? (
                <p className="mt-3 text-[13px] text-destructive" data-approve-error>
                  {v.error}
                </p>
              ) : null}
            </div>
            <div className="flex items-center justify-end gap-2 border-t bg-muted/60 px-5 py-3">
              {v.busy ? <span className="mr-auto size-3.5 animate-spin rounded-full border-2 border-muted-foreground/30 border-t-muted-foreground" /> : null}
              <Button variant="outline" disabled={v.busy} onClick={() => void deny()} data-approve-deny>
                {v.denyLabel}
              </Button>
              <Button type="submit" disabled={!v.canApprove} data-approve-submit>
                {v.approveLabel}
              </Button>
            </div>
          </form>
        ) : null}
      </DialogPopup>
    </Dialog>
  );
}

function RenameDialog({ row, labels, onClose, onRename }: { row: DeviceRow | null; labels: DevicesLabels; onClose: () => void; onRename: (name: string) => void }) {
  const [name, setName] = useState("");
  const [forId, setForId] = useState<string | null>(null);
  if (row && forId !== row.id) {
    setForId(row.id);
    setName(row.name);
  }
  return (
    <Dialog open={row !== null} onOpenChange={(o) => !o && onClose()}>
      <DialogPopup className="w-[min(380px,calc(100vw-2rem))]">
        <form
          onSubmit={(e) => {
            e.preventDefault();
            if (!name.trim()) return;
            onRename(name);
            onClose();
          }}
        >
          <div className="px-5 pt-5 pb-4">
            <DialogTitle className="text-[15px] font-semibold">{labels.renameTitle}</DialogTitle>
            <Input className="mt-3" aria-label={labels.renameTitle} value={name} autoFocus maxLength={64} onChange={(e) => setName(e.currentTarget.value)} />
          </div>
          <div className="flex justify-end gap-2 border-t bg-muted/60 px-5 py-3">
            <Button variant="outline" onClick={onClose}>
              {labels.cancel}
            </Button>
            <Button type="submit" disabled={!name.trim()}>
              {labels.renameConfirm}
            </Button>
          </div>
        </form>
      </DialogPopup>
    </Dialog>
  );
}
