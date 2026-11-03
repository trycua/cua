// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { CheckIcon, ChevronRightIcon, LoaderCircleIcon, TriangleAlertIcon } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";

import { useBridge, useConnectCloud, useNewSpaceWizard, useSpaces, type CreateSpaceArgs, type RuntimeSwitch, type WizardAction, type WizardField, type WizardTile, type WizardView } from "@/bridge";
import { CliHintBlock, useCliName } from "@/components/cli-hint";
import { Button } from "@/components/ui/button";
import { GroupedSection } from "@/components/ui/grouped";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { PromptLine } from "@/components/prompt-line";
import { toastError } from "@/components/ui/toast";
import { createCommand, openCommand } from "@/lib/cli-commands";
import { macosLimitText } from "@/lib/spaces";
import { cn } from "@/lib/utils";
import { useCreateFromWizard } from "@/hooks/use-new-space";
import { ConnectCloudDialog } from "./connect-cloud-dialog";
import { ImageField } from "./image-field";
import { MenuSelect } from "./menu-select";
import { RuntimeFixRow } from "./runtime-fix";

/**
 * New Space: System, Resources, Options, Summary, as the native app's sheet.
 * Every tile, rule, word and the create's arguments are the app core's
 * (`wizard.view`); this draws them and sends the person's choices back.
 */
export function NewSpaceDialog() {
  const wizard = useNewSpaceWizard();
  const connect = useConnectCloud();
  const { view, send } = wizard;
  const { defaultLocation, clouds = [], experiments } = wizard.env;
  // The default only counts once the clouds it may name have loaded.
  const defaultKey = [defaultLocation, experiments?.yourCloud, ...clouds.map((c) => c.name)].join(" ");

  // Until the person picks, follow the configured default (it can arrive
  // after the sheet opens, or change when a cloud is connected as default).
  useEffect(() => {
    if (wizard.open && !wizard.pinned) send({ type: "sync-default", location: defaultLocation });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [wizard.open, wizard.pinned, defaultKey]);

  return (
    <>
      <Dialog
        open={wizard.open && view !== null}
        onOpenChange={(open) => {
          if (open) return;
          // Escape closes the image suggestions, or a typed value, first.
          if (view?.imageField.open) return send({ type: "dismiss-image-suggestions" });
          if (view?.resourceEdit) return send({ type: "cancel-resource" });
          wizard.close();
        }}
      >
        <DialogPopup className="top-[9vh] w-[min(640px,calc(100vw-2rem))]" data-new-space="" data-mode={view?.mode} data-step={view?.step}>
          {view ? <WizardBody view={view} /> : null}
          {connect.open && wizard.open ? <ConnectCloudDialog /> : null}
        </DialogPopup>
      </Dialog>
      {connect.open && !wizard.open ? <ConnectCloudDialog /> : null}
    </>
  );
}

function WizardBody({ view: v }: { view: WizardView }) {
  const wizard = useNewSpaceWizard();
  const create = useCreateFromWizard();
  const { send } = wizard;
  const address = v.mode === "address";
  // Apple's two-macOS-VM limit, before the download rather than after it.
  const { core } = useBridge();
  const { data: spaces = [] } = useSpaces();
  const macosLimit = !address && v.image.os === "macos" && v.plan.placement === "local" ? macosLimitText(spaces, wizard.macosVmsRunning, core) : null;

  const primary = () => {
    if (address) return void wizard.submitAddress();
    if (v.step < 3) return send({ type: "next" });
    create();
  };

  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        if (address ? v.address.canSubmit : !macosLimit && (v.step >= 3 || v.canContinue)) primary();
      }}
    >
      <header /* design-ok: full-width dialog header rule */ className="flex items-center justify-between gap-4 border-b px-6 pt-5 pb-4">
        <DialogTitle className="text-[15px] font-semibold" data-wizard-title="">
          {v.title}
        </DialogTitle>
        {!address ? <Steps steps={v.steps} /> : null}
      </header>

      <div className="max-h-[64vh] min-h-[360px] overflow-y-auto px-6 py-5">
        {address ? (
          <AddressStep v={v} />
        ) : v.step === 0 ? (
          <SystemStep v={v} />
        ) : v.step === 1 ? (
          <ResourcesStep v={v} />
        ) : v.step === 2 ? (
          <OptionsStep v={v} />
        ) : (
          <SummaryStep v={v} />
        )}
        {macosLimit ? (
          <p role="alert" data-macos-limit="" className="mt-3 px-1 text-xs text-destructive">
            {macosLimit}
          </p>
        ) : null}
      </div>

      <footer /* design-ok: full-width dialog footer */ className="flex items-center gap-2 border-t px-6 py-3">
        <Button variant="secondary" data-wizard-cancel="" onClick={wizard.close}>
          {v.labels.cancel}
        </Button>
        <span className="flex-1" />
        {address ? (
          <>
            <Button variant="secondary" data-wizard-back="" onClick={() => send({ type: "hide-address" })}>
              {v.labels.back}
            </Button>
            <Button type="submit" data-wizard-primary="" disabled={!v.address.canSubmit}>
              {v.address.submitLabel}
            </Button>
          </>
        ) : (
          <>
            {v.showBack ? (
              <Button variant="secondary" data-wizard-back="" onClick={() => send({ type: "back" })}>
                {v.labels.back}
              </Button>
            ) : null}
            <Button type="submit" data-wizard-primary="" disabled={Boolean(macosLimit) || (v.step < 3 && !v.canContinue)}>
              {v.primaryLabel}
            </Button>
          </>
        )}
      </footer>
    </form>
  );
}

function Steps({ steps }: { steps: WizardView["steps"] }) {
  return (
    <ol aria-label="Steps" className="flex items-center gap-1 text-xs">
      {steps.map((s, i) => (
        <li key={s.label} data-step-state={s.state} aria-current={s.state === "current" ? "step" : undefined} className="flex items-center gap-1">
          {i > 0 ? <ChevronRightIcon className="size-3 text-muted-foreground/50" /> : null}
          <span
            className={cn(
              "flex size-4 items-center justify-center rounded-full text-2xs font-medium tabular-nums",
              s.state === "current" ? "bg-brand text-white" : s.state === "done" ? "bg-foreground/10 text-foreground" : "bg-muted text-muted-foreground",
            )}
          >
            {s.state === "done" ? <CheckIcon className="size-2.5" strokeWidth={3} /> : i + 1}
          </span>
          <span className={s.state === "current" ? "font-medium text-foreground" : "text-muted-foreground"}>{s.label}</span>
        </li>
      ))}
    </ol>
  );
}

/* ---- Layout ---------------------------------------------------------------- */

function Group({ children, className, ...rest }: { children: ReactNode; className?: string } & Record<`data-${string}`, string>) {
  // Not clipped: the image field's suggestions float over the rows below the card's edge.
  return (
    <GroupedSection className={className} contentClassName="overflow-visible" {...rest}>
      {children}
    </GroupedSection>
  );
}

/** A label on the left and its control on the right; `stacked` puts the control under it. */
function Row({ id, label, htmlFor, children, below, stacked }: { id: string; label: string; htmlFor?: string; children: ReactNode; below?: ReactNode; stacked?: boolean }) {
  return (
    <div data-field={id} className="px-4 py-2.5">
      <div className={cn(stacked ? "space-y-2" : "flex min-h-8 items-center justify-between gap-6")}>
        <label htmlFor={htmlFor} className="block shrink-0 text-[13px]">
          {label}
        </label>
        <div className={cn(stacked ? "" : "flex min-w-0 justify-end")}>{children}</div>
      </div>
      {below}
    </div>
  );
}

/** "Use built-in Lume" beside "Run on"'s error: switches the runtime
 * setting, then This Mac's runtimes are read again. */
function RuntimeSwitchButton({ change }: { change: RuntimeSwitch }) {
  const { applyRuntimeSwitch } = useNewSpaceWizard();
  const [switching, setSwitching] = useState(false);
  return (
    <span title={change.detail} className="mt-1 inline-flex shrink-0">
      <Button
        size="sm"
        variant="secondary"
        data-runtime-switch={change.setting}
        disabled={switching}
        onClick={() => {
          setSwitching(true);
          void applyRuntimeSwitch(change)
            .catch(toastError("Couldn't switch the runtime"))
            .finally(() => setSwitching(false));
        }}
      >
        {switching ? <LoaderCircleIcon className="size-3.5 animate-spin" /> : null}
        {change.label}
      </Button>
    </span>
  );
}

function FieldError({ id, children }: { id: string; children: ReactNode }) {
  return (
    <p role="alert" data-field-error={id} className="mt-1.5 text-xs text-destructive">
      {children}
    </p>
  );
}

function Note({ children, ...rest }: { children: ReactNode } & Record<`data-${string}`, string>) {
  return (
    <p className="mt-1.5 text-xs text-muted-foreground" {...rest}>
      {children}
    </p>
  );
}

function Tiles({ id, label, tiles, onPick }: { id: string; label: string; tiles: WizardTile[]; onPick: (id: string) => void }) {
  return (
    <div role="group" aria-label={label} className="inline-flex h-7 items-center gap-0.5 rounded-lg bg-muted p-0.5">
      {tiles.map((t) => (
        <button
          key={t.id}
          type="button"
          data-tile={`${id}:${t.id}`}
          aria-pressed={t.pressed}
          disabled={!t.enabled}
          title={t.detail || undefined}
          onClick={() => onPick(t.id)}
          // The chosen tile is filled (the native segmented control's
          // selection), not only outlined by focus.
          className="inline-flex h-6 items-center rounded-lg px-2.5 text-xs font-medium text-muted-foreground outline-none transition-colors hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60 disabled:opacity-40 disabled:hover:text-muted-foreground aria-pressed:bg-primary aria-pressed:text-primary-foreground aria-pressed:shadow-xs aria-pressed:hover:text-primary-foreground"
        >
          {t.title}
        </button>
      ))}
    </div>
  );
}

const fieldOf = (v: WizardView, id: string): WizardField | undefined => v.fields.find((f) => f.id === id);
const labelOf = (v: WizardView, id: string) => fieldOf(v, id)?.label ?? "";

/* ---- System ---------------------------------------------------------------- */

// "Add your cloud account" is the last entry of Run on, not a row of its own.
const LINKS = new Set(["connect-cloud"]);

function SystemStep({ v }: { v: WizardView }) {
  const { send } = useNewSpaceWizard();
  const firstAdvanced = v.fields.findIndex((f) => f.advanced);
  const main = firstAdvanced < 0 ? v.fields : v.fields.slice(0, firstAdvanced);
  const advanced = v.fields.filter((f) => f.advanced);
  const rest = firstAdvanced < 0 ? [] : v.fields.slice(firstAdvanced).filter((f) => !f.advanced);
  const rows = (fields: WizardField[]) => fields.filter((f) => !LINKS.has(f.id));

  return (
    <div className="space-y-4">
      <Group>
        {rows(main).map((f) => (
          <SystemRow key={f.id} field={f} v={v} />
        ))}
      </Group>
      {advanced.length ? (
        <div>
          <button
            type="button"
            data-advanced-toggle=""
            aria-expanded={v.advanced}
            onClick={() => send({ type: "toggle-advanced" })}
            className="flex items-center gap-1 px-1 text-xs font-semibold text-muted-foreground outline-none hover:text-foreground focus-visible:text-foreground"
          >
            <ChevronRightIcon className={cn("size-3.5 transition-transform", v.advanced && "rotate-90")} />
            {v.labels.advanced}
          </button>
          {v.advanced ? (
            <Group className="mt-2">
              {advanced.map((f) => (
                <SystemRow key={f.id} field={f} v={v} />
              ))}
            </Group>
          ) : null}
        </div>
      ) : null}
      {rows(rest).length ? (
        <Group>
          {rows(rest).map((f) => (
            <SystemRow key={f.id} field={f} v={v} />
          ))}
        </Group>
      ) : null}
    </div>
  );
}

const ADD_CLOUD = "add:cloud";

function SystemRow({ field: f, v }: { field: WizardField; v: WizardView }) {
  const { send } = useNewSpaceWizard();
  const connect = useConnectCloud();
  const error = f.error ? <FieldError id={f.id}>{f.error}</FieldError> : null;
  switch (f.id) {
    case "os":
      return (
        <Row id="os" label={f.label} below={error}>
          <Tiles id="os" label={f.label} tiles={v.osTiles} onPick={(os) => send({ type: "choose-os", os })} />
        </Row>
      );
    case "image":
      return (
        <Row id="image" label={f.label} htmlFor="ns-image" stacked>
          <ImageField label={f.label} placeholder={f.placeholder} view={v.imageField} send={send} />
        </Row>
      );
    case "placement":
      return (
        <Row
          id="placement"
          label={f.label}
          below={
            <>
              {error && v.runtimeSwitch ? (
                <div className="flex items-start justify-between gap-3">
                  {error}
                  <RuntimeSwitchButton change={v.runtimeSwitch} />
                </div>
              ) : (
                error
              )}
              {error && v.placementPrompt ? <PromptLine id="add-machine" label="Or ask your agent" text={v.placementPrompt} /> : null}
              {error && !v.runtimeSwitch && v.plan.placement === "local" ? <RuntimeFixRow /> : null}
              {v.placementHint ? <Note data-placement-hint="">{v.placementHint}</Note> : null}
            </>
          }
        >
          <MenuSelect
            aria-label={f.label}
            dataName="run-on"
            value={v.placementId}
            items={[
              ...v.placements.map((o) => ({ value: o.id, label: o.label, detail: o.detail, disabled: !o.enabled, group: o.group })),
              // Only with the Your cloud experiment (the core offers the field); it opens the connect sheet.
              ...(v.fields.some((x) => x.id === "connect-cloud") ? [{ value: ADD_CLOUD, label: "Add your cloud account\u2026", detail: "AWS, Google Cloud, Hetzner and others", group: ADD_CLOUD }] : []),
            ]}
            onValueChange={(on) => (on === ADD_CLOUD ? connect.show() : send({ type: "choose-placement", on }))}
          />
        </Row>
      );
    case "kind":
      return (
        <Row
          id="kind"
          label={f.label}
          below={error}
        >
          <Tiles id="kind" label={f.label} tiles={v.kindTiles} onPick={(kind) => send({ type: "choose-kind", kind })} />
        </Row>
      );
    case "runtime":
      return (
        <Row id="runtime" label={f.label} below={error}>
          <MenuSelect
            aria-label={f.label}
            dataName="runtime"
            value={v.runtime}
            disabled={!v.runtimeEnabled}
            items={v.runtimes.map((r) => ({ value: r.value, label: r.label }))}
            onValueChange={(runtime) => send({ type: "set-runtime", runtime })}
          />
        </Row>
      );
    default:
      return null;
  }
}

/* ---- Resources ------------------------------------------------------------- */

/** The slider's number: click it to type a value. The core checks it against
 * the slider's own limits (`edit-resource` / `set-resource-text` /
 * `commit-resource`); Return or leaving the field uses it, Escape keeps the
 * slider's value. */
function SliderValue({ id, label, text, edit }: { id: "cpus" | "memory" | "disk"; label: string; text: string; edit: WizardView["resourceEdit"] }) {
  const { send } = useNewSpaceWizard();
  if (edit?.field === id) {
    return (
      <input
        id={`ns-${id}-value`}
        data-value-input={id}
        autoFocus
        inputMode="numeric"
        spellCheck={false}
        autoComplete="off"
        aria-label={label}
        aria-invalid={Boolean(edit.error)}
        aria-describedby={edit.error ? `ns-${id}-error` : undefined}
        value={edit.text}
        onFocus={(e) => e.currentTarget.select()}
        onChange={(e) => send({ type: "set-resource-text", text: e.target.value })}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            send({ type: "commit-resource" });
          } else if (e.key === "Escape") {
            // Keeps the sheet open: only the typed value is dropped.
            e.preventDefault();
            e.stopPropagation();
            send({ type: "cancel-resource" });
          }
        }}
        onBlur={() => send({ type: "commit-resource" })}
        className="h-6 w-16 rounded-lg border border-input bg-transparent px-1.5 text-right text-[13px] tabular-nums outline-none focus-visible:ring-2 focus-visible:ring-ring/60 aria-invalid:border-destructive" // design-ok: inline number field outline, matches Input
      />
    );
  }
  return (
    <button
      type="button"
      data-value-text={id}
      title={`Type a value for ${label.toLowerCase()}`}
      onClick={() => send({ type: "edit-resource", field: id })}
      className="w-16 cursor-text rounded-lg px-1.5 text-right text-[13px] tabular-nums hover:bg-muted/60 focus-visible:ring-2 focus-visible:ring-ring/60 focus-visible:outline-none"
    >
      {text}
    </button>
  );
}

function Slider({
  id,
  label,
  value,
  min,
  max,
  text,
  title,
  edit,
  onChange,
}: {
  id: "cpus" | "memory" | "disk";
  label: string;
  value: number;
  min: number;
  max: number;
  text: string;
  title?: string;
  edit: WizardView["resourceEdit"];
  onChange: (n: number) => void;
}) {
  return (
    <div className="flex w-72 items-center gap-3" title={title}>
      <input
        id={`ns-${id}`}
        type="range"
        min={min}
        max={Math.max(max, min + 1)}
        step={1}
        value={value}
        aria-label={label}
        aria-valuetext={text}
        onChange={(e) => onChange(Number(e.target.value))}
        className="h-1 flex-1 accent-brand"
      />
      <SliderValue id={id} label={label} text={text} edit={edit} />
    </div>
  );
}

/** Why a typed value cannot be used, under its slider (the field's error). */
function typedError(v: WizardView, id: string): ReactNode {
  const error = v.resourceEdit?.field === id ? v.resourceEdit.error : null;
  return error ? (
    <p role="alert" id={`ns-${id}-error`} data-field-error={id} className="mt-1.5 text-right text-xs text-destructive">
      {error}
    </p>
  ) : null;
}

function ResourcesStep({ v }: { v: WizardView }) {
  const { send } = useNewSpaceWizard();
  const { data } = useBridge();
  const openExternal = (url: string) => void data?.call("session.openExternal", { url }).catch(() => window.open(url, "_blank", "noopener"));
  // In your cloud the machine type sets the size: no sliders.
  const sized = Boolean(fieldOf(v, "cpus") || fieldOf(v, "memory") || v.diskEditable || v.gpu);
  return (
    <div className="space-y-4">
      {sized ? (
        <Group>
          {fieldOf(v, "cpus") ? (
            <Row id="cpus" label={labelOf(v, "cpus")} htmlFor="ns-cpus" below={typedError(v, "cpus")}>
              <Slider
                id="cpus"
                label={labelOf(v, "cpus")}
                value={v.cpus}
                min={v.minCpus}
                max={v.maxCpus}
                text={v.cpusText}
                edit={v.resourceEdit}
                onChange={(cpus) => send({ type: "set-cpus", cpus })}
              />
            </Row>
          ) : null}
          {fieldOf(v, "memory") ? (
            <Row id="memory" label={labelOf(v, "memory")} htmlFor="ns-memory" below={typedError(v, "memory")}>
              <Slider
                id="memory"
                label={labelOf(v, "memory")}
                value={v.memoryGb}
                min={v.minMemoryGb}
                max={v.maxMemoryGb}
                text={v.memoryText}
                edit={v.resourceEdit}
                onChange={(memoryGb) => send({ type: "set-memory", memoryGb })}
              />
            </Row>
          ) : null}
          {v.diskEditable ? (
            <Row
              id="disk"
              label={labelOf(v, "disk")}
              htmlFor="ns-disk"
              below={
                v.resourceEdit?.field === "disk" && v.resourceEdit.error ? (
                  typedError(v, "disk")
                ) : v.diskNote || v.diskResetLabel ? (
                  <div className="mt-1.5 flex items-baseline justify-between gap-4">
                    {v.diskNote ? (
                      <p data-disk-note="" className="text-xs text-muted-foreground">
                        {v.diskNote}
                      </p>
                    ) : (
                      <span />
                    )}
                    {v.diskResetLabel ? (
                      <Button variant="secondary" size="sm" data-disk-reset="" onClick={() => send({ type: "reset-disk" })}>
                        {v.diskResetLabel}
                      </Button>
                    ) : null}
                  </div>
                ) : null
              }
            >
              <Slider
                id="disk"
                label={labelOf(v, "disk")}
                value={v.diskGb}
                min={v.minDiskGb}
                max={v.maxDiskGb}
                text={v.diskText}
                title={v.diskHelp ?? undefined}
                edit={v.resourceEdit}
                onChange={(diskGb) => send({ type: "set-disk", diskGb })}
              />
            </Row>
          ) : null}
          {v.gpu ? (
            <div data-field="gpu" className="px-4 py-2.5">
              <div className="flex min-h-8 items-center justify-between gap-6">
                {/* The label names the checkbox; why it is off describes it (not its name). */}
                <label className="flex items-center gap-2 text-[13px]">
                  <Checkbox
                    data-gpu=""
                    aria-labelledby="ns-gpu-label"
                    aria-describedby={!v.gpu.enabled && v.gpu.reason ? "ns-gpu-reason" : undefined}
                    checked={v.gpu.on}
                    disabled={!v.gpu.enabled}
                    onCheckedChange={(on) => send({ type: "set-gpu", on })}
                  />
                  <span id="ns-gpu-label" data-gpu-label="" className={cn(!v.gpu.enabled && "text-muted-foreground")}>
                    {v.gpu.label}
                  </span>
                </label>
                {v.gpu.learnMoreUrl ? (
                  <button type="button" className="text-xs text-brand-strong hover:underline" onClick={() => openExternal(v.gpu!.learnMoreUrl!)}>
                    {v.gpu.learnMoreLabel}
                  </button>
                ) : null}
              </div>
              {!v.gpu.enabled && v.gpu.reason ? (
                <p id="ns-gpu-reason" data-gpu-reason="" className="mt-1.5 text-xs text-muted-foreground">
                  {v.gpu.reason}
                </p>
              ) : null}
            </div>
          ) : null}
        </Group>
      ) : null}
      {v.price ? (
        <p data-price="" className="px-1 text-[13px] text-muted-foreground tabular-nums">
          {v.price}
        </p>
      ) : null}
      {v.resourceFacts.length ? (
        <Group data-facts="">
          {v.resourceFacts.map((fact) => (
            <div key={fact.id} data-fact={fact.id} className="flex items-baseline justify-between gap-6 px-4 py-2 text-[13px]">
              <span className="shrink-0 text-muted-foreground">{fact.label}</span>
              <span className="flex min-w-0 items-center gap-1.5 text-right">
                <span data-fact-value="">{fact.value}</span>
                {fact.symbol ? (
                  <span role="img" aria-label={fact.help ?? undefined} title={fact.help ?? undefined}>
                    <TriangleAlertIcon className="size-3.5 text-warning" />
                  </span>
                ) : null}
              </span>
            </div>
          ))}
        </Group>
      ) : null}
      {v.resourcesError ? (
        <p role="alert" data-resources-error="" className="px-1 text-xs text-destructive">
          {v.resourcesError}
        </p>
      ) : null}
    </div>
  );
}

/* ---- Options and Summary --------------------------------------------------- */

function OptionsStep({ v }: { v: WizardView }) {
  const { send } = useNewSpaceWizard();
  const name = fieldOf(v, "name");
  return (
    <div className="space-y-4">
      <Group>
        <Row id="name" label={labelOf(v, "name")} htmlFor="ns-name" stacked below={v.nameError ? <FieldError id="name">{v.nameError}</FieldError> : null}>
          <Input
            id="ns-name"
            data-name-input=""
            value={v.name}
            placeholder={name?.placeholder ?? undefined}
            spellCheck={false}
            autoComplete="off"
            aria-invalid={v.nameInvalid}
            className="aria-invalid:border-destructive"
            onChange={(e) => send({ type: "set-name", name: e.target.value })}
          />
        </Row>
        <div data-field="open-when-ready" className="px-4 py-2.5">
          <label className="flex min-h-8 items-center gap-2 text-[13px]">
            <Checkbox data-open-when-ready="" checked={v.openWhenReady} onCheckedChange={(on) => send({ type: "set-open-when-ready", on })} />
            {labelOf(v, "open-when-ready")}
          </label>
        </div>
      </Group>
      {v.streamNote ? (
        <p role="note" data-stream-note="" className="px-1 text-xs text-muted-foreground">
          {v.streamNote}
        </p>
      ) : null}
    </div>
  );
}

/** The Summary rows only a developer needs (`wizard::SUMMARY_ADVANCED`): they
 * sit under "Advanced", the same disclosure the System step uses. */
const SUMMARY_ADVANCED = new Set(["Image", "Kind", "Architecture", "Runtime"]);

function SummaryRow({ fact }: { fact: { label: string; value: string } }) {
  return (
    <div data-summary-row="" className="flex items-baseline justify-between gap-6 px-4 py-2 text-[13px]">
      <span className="shrink-0 text-muted-foreground">{fact.label}</span>
      <span className={cn("min-w-0 text-right break-all", fact.label === "Image" && "font-mono text-xs")}>{fact.value}</span>
    </div>
  );
}

function SummaryStep({ v }: { v: WizardView }) {
  const { send } = useNewSpaceWizard();
  const plain = v.summary.filter((f) => !SUMMARY_ADVANCED.has(f.label));
  const technical = v.summary.filter((f) => SUMMARY_ADVANCED.has(f.label));
  return (
    <>
      <Group data-summary="">
        {plain.map((fact) => (
          <SummaryRow key={fact.label} fact={fact} />
        ))}
      </Group>
      {technical.length ? (
        <div className="mt-4">
          <button
            type="button"
            data-advanced-toggle=""
            aria-expanded={v.advanced}
            onClick={() => send({ type: "toggle-advanced" })}
            className="flex items-center gap-1 px-1 text-xs font-semibold text-muted-foreground outline-none hover:text-foreground focus-visible:text-foreground"
          >
            <ChevronRightIcon className={cn("size-3.5 transition-transform", v.advanced && "rotate-90")} />
            {v.labels.advanced}
          </button>
          {v.advanced ? (
            <Group className="mt-2" data-summary-advanced="">
              {technical.map((fact) => (
                <SummaryRow key={fact.label} fact={fact} />
              ))}
            </Group>
          ) : null}
        </div>
      ) : null}
      <SummaryCommands v={v} />
    </>
  );
}

/** What Create does, as terminal commands (the create the core would make,
 * then opening it when "Open when it's ready" is on); only with the CLI
 * installed. */
function SummaryCommands({ v }: { v: WizardView }) {
  const { core } = useBridge();
  const cli = useCliName();
  if (!cli) return null;
  let args: CreateSpaceArgs | undefined;
  try {
    args = core.tryCall<CreateSpaceArgs>("wizard.createArgs", { plan: v.plan });
  } catch {
    args = undefined;
  }
  const create = args ? createCommand(cli, args) : null;
  if (!args || !create) return null;
  const open = v.plan.openWhenReady ? openCommand(cli, args) : null;
  return <CliHintBlock commands={open ? [create, open] : [create]} />;
}

/* ---- Connect by address ---------------------------------------------------- */

function AddressStep({ v }: { v: WizardView }) {
  const { send, address } = useNewSpaceWizard();
  const field = (id: string, value: string, action: (text: string) => WizardAction, type = "text") => {
    const f = fieldOf(v, id);
    if (!f) return null;
    return (
      <Row id={id} label={f.label} htmlFor={`ns-${id}`} stacked>
        <Input
          id={`ns-${id}`}
          data-address-input={id}
          type={type}
          value={value}
          placeholder={f.placeholder ?? undefined}
          spellCheck={false}
          autoComplete="off"
          aria-invalid={id === "address" && v.address.showInvalid}
          className="aria-invalid:border-destructive"
          onChange={(e) => send(action(e.target.value))}
        />
      </Row>
    );
  };
  return (
    <div className="space-y-4">
      <Group>
        {field("address", address.url, (url) => ({ type: "set-address", url }))}
        {field("token", address.token, (token) => ({ type: "set-token", token }), "password")}
        {field("address-name", address.name, (name) => ({ type: "set-address-name", name }))}
      </Group>
      {v.price ? (
        <p data-price="" className="px-1 text-[13px] text-muted-foreground">
          {v.price}
        </p>
      ) : null}
      {v.address.error ? (
        <p role="alert" data-address-error="" className="px-1 text-xs text-destructive">
          {v.address.error}
        </p>
      ) : null}
    </div>
  );
}
