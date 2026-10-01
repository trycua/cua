// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Fragment, useEffect, useMemo, useState } from "react";

import { core } from "../../core";
import { NO_EXPERIMENTS, type Experiments } from "../../model/experiments";
import type { Placement, SandboxImage } from "../../model/images";
import type { Runtime } from "../../model/types";
import type { CloudPricing, GpuChoice } from "../../native/fleet";
import type { LocalStorage } from "../../native/local";
import { telemetryBridge, type TelemetryBridge } from "../../native/telemetry";
import { ImageField, type ImageFieldView } from "./ImageField";
import { Sym } from "./Sym";

/** What the wizard asks the app to create (the app core's `CreatePlan`). */
export interface CreatePlan {
  placement: Placement;
  /** "Your cloud": the connected cloud's word (`aws`). */
  cloud?: string;
  /** One of your machines: its id (`on="host:<id>"`). */
  host?: string;
  /** The image; its `variant` is the kind. */
  image: SandboxImage;
  /** The engine: `auto` unless chosen under Advanced. Always one the image
   * can run in `placement`. */
  runtime: Runtime;
  name?: string;
  /** This Mac only: vCPUs and memory. Cua Cloud sizes its own Spaces. */
  cpus?: number;
  memoryMb?: number;
  /** This Mac's VMs only: a disk larger than the image's, in GB. */
  diskGb?: number;
  openWhenReady: boolean;
  /** Open the desktop once created (asked for, and the image streams). */
  openDesktop: boolean;
  /** A GPU option of the runtime (`paravirtual`), when checked and offered. */
  gpu?: string;
}

export interface NewSpaceWizardProps {
  /** Where "Run on" starts: the configured default location (Settings,
   * `cua config set default.on`). */
  defaultLocation?: Placement;
  /** Cua Cloud needs an account (signed in, or client credentials). */
  cloudAvailable: boolean;
  /** Local needs a container backend (docker / runsc) or Lume. */
  localAvailable: boolean;
  /** Why local is unavailable, shown on the disabled tile. */
  localReason?: string;
  /** Ready local backends (`local_status`); when given, a local image also
   * needs the backend its runtime uses (containers, QEMU or Lume). */
  localBackends?: readonly string[];
  /** Starts the create. The wizard closes as soon as this is called; the
   * roster shows the Space starting. */
  onCreate: (plan: CreatePlan) => void;
  /** "Connect by address": awaited so the handshake error shows inline. */
  onAddByAddress: (url: string, token?: string, name?: string) => Promise<unknown>;
  onCancel: () => void;
  /** Usage telemetry (the shell's; nothing outside it). */
  telemetry?: TelemetryBridge;
  /** Upper bound for the vCPU slider. */
  maxCpus?: number;
  /** This Mac's architecture (`arm64`, `amd64`), from `local_status`. */
  hostArch?: string;
  /** Free space where local Spaces are written, and what is pulled. */
  storage?: LocalStorage | null;
  /** This account's Cua Cloud rates (no cloud estimate without). */
  cloudPricing?: CloudPricing | null;
  /** The GPU option of each runtime that has one (`gpu_support`); a
   * runtime not listed gets no GPU row. */
  gpus?: readonly GpuChoice[] | null;
  /** Opens a page in the browser (the GPU row's "Learn more"). */
  onOpenExternal?: (url: string) => void;
  /** Actions applied once when the sheet opens (start views for captures). */
  startActions?: readonly ({ type: string } & Record<string, unknown>)[];
  /** The user's connected clouds (the app core's `ConnectedCloud`s); the
   * menu lists them only with the "Your cloud" experiment. */
  clouds?: readonly ConnectedCloud[];
  /** The user's machines that provide Spaces (`list_hosts`). */
  hosts?: readonly SpaceHost[];
  /** Settings, Experiments ("Your cloud" decides whether clouds show). */
  experiments?: Experiments;
  /** "Connect a cloud...": opens the Connect a cloud sheet. */
  onConnectCloud?: () => void;
}

/** One of the user's machines that provides Spaces (`wizard::SpaceHost`). */
export interface SpaceHost {
  id: string;
  name: string;
  via: "relay" | "direct" | string;
  online: boolean;
  os: string;
  limits: { resource: string; used: number; limit: number; reason: string }[];
}

/** One entry of the "Run on" menu (`wizard::PlacementOption`). */
interface PlacementOption {
  /** What choosing it sets as `on`: `local`, `host:<id>`, `aws`, ... */
  id: string;
  label: string;
  /** `this-mac`, `hosts`, `clouds`. */
  group: string;
  selected: boolean;
  enabled: boolean;
  detail: string;
}

/** A connected cloud as the app core reads it (`wizard::ConnectedCloud`). */
export interface ConnectedCloud {
  name: string;
  title: string;
  label: string;
  isDefault: boolean;
  ttlHours: number;
  offers: {
    image: string;
    kind: string;
    supported: boolean;
    reason: string;
    machineType: string;
    usdPerHour: number;
  }[];
}

/** `host:port` or `http(s)://host:port`; a shape check only (the app core's). */
export function looksLikeAddress(value: string): boolean {
  return core("wizard.looksLikeAddress", { value });
}

/** The SDK's handshake errors, in words a person can act on. */
export function friendlyAddError(raw: string): string {
  return core("wizard.friendlyAddError", { raw });
}

/* The app core's wizard state and view (`cua-spaces-app-core::wizard`). */
type WizardState = Record<string, unknown>;
interface Tile {
  id: string;
  title: string;
  detail: string;
  pressed: boolean;
  enabled: boolean;
}
/** One field of the current step, in order (the core's `WizardField`). */
interface WizardField {
  id: string;
  label: string;
  placeholder: string | null;
  error: string | null;
  advanced: boolean;
}
/** One line of the Resources step (the core's `WizardFact`). */
interface WizardFact {
  id: string;
  label: string;
  value: string;
  /** SF Symbol name after the value (`exclamationmark.triangle`). */
  symbol?: string;
  help?: string;
}
interface WizardView {
  mode: "create" | "address";
  step: number;
  steps: { label: string; state: "done" | "current" | "todo" }[];
  title: string;
  osTiles: Tile[];
  imageField: ImageFieldView;
  image: SandboxImage;
  /** The "Run on" menu: This Mac, your machines, your clouds. */
  placements: PlacementOption[];
  /** The chosen entry's id. */
  placementId: string;
  /** The connected cloud "Your cloud" creates in. */
  cloud: string | null;
  /** The machine of yours it creates on. */
  host: string | null;
  placementError: string | null;
  advanced: boolean;
  kindTiles: Tile[];
  runtimes: { value: Runtime; label: string }[];
  runtime: Runtime;
  runtimeEnabled: boolean;
  cpus: number;
  minCpus: number;
  maxCpus: number;
  minMemoryGb: number;
  maxMemoryGb: number;
  cpusText: string;
  memoryGb: number;
  memoryText: string;
  diskEditable: boolean;
  diskGb: number;
  minDiskGb: number;
  maxDiskGb: number;
  diskText: string;
  /** One quiet line under the disk slider (sparse disk, resize). */
  diskNote: string | null;
  /** "Reset" while the chosen disk is not the image's. */
  diskResetLabel: string | null;
  /** The disk slider's tooltip. */
  diskHelp: string | null;
  /** `About $0.18/hour` (cloud); null on the user's own hardware and
   * without cloud rates. */
  price: string | null;
  resourceFacts: WizardFact[];
  resourcesError: string | null;
  /** The GPU row (Resources), where the runtime offers a GPU. */
  gpu: GpuRow | null;
  name: string;
  nameInvalid: boolean;
  nameError: string | null;
  openWhenReady: boolean;
  streamNote: string | null;
  summary: { label: string; value: string }[];
  canContinue: boolean;
  showBack: boolean;
  primaryLabel: string;
  plan: CreatePlan;
  address: {
    valid: boolean;
    showInvalid: boolean;
    canSubmit: boolean;
    submitLabel: string;
    error: string | null;
    submit: { url: string; token: string | null; name: string | null } | null;
  };
  fields: WizardField[];
  labels: { cancel: string; back: string; advanced: string };
}
type WizardAction = { type: string } & Record<string, unknown>;
/** The Resources step's GPU row (the core's `GpuRow`). */
interface GpuRow {
  label: string;
  on: boolean;
  enabled: boolean;
  reason: string | null;
  learnMoreLabel: string;
  learnMoreUrl: string | null;
}

/**
 * New Space, as a step-by-step assistant in the manner of a VM creation
 * wizard: pick the system and image, size it, name it, review, create.
 * The state machine is the app core's; this renders it.
 */
export function NewSpaceWizard({
  defaultLocation = "local",
  cloudAvailable,
  localAvailable,
  localReason,
  localBackends,
  onCreate,
  onAddByAddress,
  onCancel,
  telemetry: telemetryProp,
  maxCpus = Math.max(2, Math.min(16, globalThis.navigator?.hardwareConcurrency || 8)),
  hostArch,
  storage,
  cloudPricing,
  gpus,
  onOpenExternal,
  startActions,
  clouds,
  hosts,
  experiments,
  onConnectCloud,
}: NewSpaceWizardProps) {
  const env = useMemo(
    () => ({
      defaultLocation,
      cloudAvailable,
      localAvailable,
      localReason: localReason ?? null,
      localBackends: localBackends ? [...localBackends] : null,
      maxCpus,
      hostArch: hostArch ?? null,
      storage: storage ?? null,
      cloudPricing: cloudPricing ?? null,
      clouds: clouds ? [...clouds] : [],
      hosts: hosts ? [...hosts] : [],
      experiments: experiments ?? NO_EXPERIMENTS,
      gpus: gpus ? [...gpus] : null,
    }),
    [
      defaultLocation,
      cloudAvailable,
      localAvailable,
      localReason,
      localBackends,
      maxCpus,
      hostArch,
      storage,
      cloudPricing,
      clouds,
      hosts,
      experiments,
      gpus,
    ],
  );
  const [state, setState] = useState<WizardState>(() =>
    (startActions ?? []).reduce<WizardState>(
      (s, action) => core<WizardState>("wizard.reduce", { state: s, action, env }),
      core<WizardState>("wizard.initial", { env }),
    ),
  );
  // The panel opened, was cancelled, or created a Space (the SwiftUI app
  // records the same).
  const telemetry = useMemo(() => telemetryProp ?? telemetryBridge(), [telemetryProp]);
  useEffect(() => {
    telemetry.recordSignals([{ type: "space-wizard", action: "opened" }]);
  }, [telemetry]);
  const cancel = () => {
    telemetry.recordSignals([{ type: "space-wizard", action: "cancelled" }]);
    onCancel();
  };
  const dispatch = (action: WizardAction) =>
    setState((s) => core<WizardState>("wizard.reduce", { state: s, action, env }));
  // Until the person picks, follow the configured default (it can arrive
  // after the sheet opens); the core ignores it once they picked.
  useEffect(() => {
    setState((s) => core<WizardState>("wizard.reduce", { state: s, action: { type: "sync-default", location: defaultLocation }, env }));
  }, [defaultLocation, env]);
  const v = core<WizardView>("wizard.view", { state, env });
  const step = v.step;

  if (v.mode === "address") {
    return (
      <div className="dw-sheet" role="dialog" aria-label="Connect by address">
        <AddByAddress
          view={v}
          address={state.address as { url: string; token: string; name: string }}
          dispatch={dispatch}
          onAdd={onAddByAddress}
          onDone={onCancel}
        />
      </div>
    );
  }

  return (
    <div className="dw-sheet" role="dialog" aria-label="New Space">
      <div className="wz">
        <ol className="wz-steps" aria-label="Steps">
          {v.steps.map((s, index) => (
            <li
              key={s.label}
              className="wz-step"
              data-state={s.state}
              aria-current={s.state === "current" ? "step" : undefined}
            >
              <span className="wz-step-num">{s.state === "done" ? <Sym name="checkmark" size={11} /> : index + 1}</span>
              <span className="wz-step-label">{s.label}</span>
            </li>
          ))}
        </ol>

        <div className="wz-body">
          {step === 0 && (
            <div className="wz-stack">
              <h2 className="wz-title">{v.title}</h2>
              {v.fields.map((field, index) => {
                if (field.advanced) {
                  // "Advanced" holds every advanced field, where the first one is.
                  if (v.fields.findIndex((f) => f.advanced) !== index) return null;
                  return (
                    <div className="wz-advanced" key="advanced">
                      <button
                        type="button"
                        className="dw-btn-link"
                        aria-expanded={v.advanced}
                        onClick={() => dispatch({ type: "toggle-advanced" })}
                      >
                        {v.labels.advanced}
                      </button>
                      {v.advanced && (
                        <div className="wz-stack" role="group" aria-label={v.labels.advanced}>
                          {v.fields.filter((f) => f.advanced).map((f) => (
                            <SystemField key={f.id} field={f} view={v} dispatch={dispatch} />
                          ))}
                        </div>
                      )}
                    </div>
                  );
                }
                return (
                  <SystemField
                    key={field.id}
                    field={field}
                    view={v}
                    dispatch={dispatch}
                    onConnectCloud={onConnectCloud}
                  />
                );
              })}
            </div>
          )}

          {step === 1 && (
            <div className="wz-stack">
              <h2 className="wz-title">{v.title}</h2>
              {field(v, "cpus") && (
                <label className="dw-field">
                  <span>{label(v, "cpus")}</span>
                  <span className="wz-slider">
                    <input
                      type="range"
                      min={v.minCpus}
                      max={v.maxCpus}
                      step={1}
                      value={v.cpus}
                      aria-label={label(v, "cpus")}
                      onChange={(event) => dispatch({ type: "set-cpus", cpus: Number(event.target.value) })}
                    />
                    <output>{v.cpusText}</output>
                  </span>
                </label>
              )}
              {field(v, "memory") && (
                <label className="dw-field">
                  <span>{label(v, "memory")}</span>
                  <span className="wz-slider">
                    <input
                      type="range"
                      min={v.minMemoryGb}
                      max={v.maxMemoryGb}
                      step={1}
                      value={v.memoryGb}
                      aria-label={label(v, "memory")}
                      onChange={(event) => dispatch({ type: "set-memory", memoryGb: Number(event.target.value) })}
                    />
                    <output>{v.memoryText}</output>
                  </span>
                </label>
              )}
                {v.diskEditable && (
                  <div className="wz-disk">
                    <label className="dw-field">
                      <span>{label(v, "disk")}</span>
                      <span className="wz-slider" title={v.diskHelp ?? undefined}>
                        <input
                          type="range"
                          min={v.minDiskGb}
                          max={v.maxDiskGb}
                          step={1}
                          value={v.diskGb}
                          aria-label={label(v, "disk")}
                          onChange={(event) => dispatch({ type: "set-disk", diskGb: Number(event.target.value) })}
                        />
                        <output>{v.diskText}</output>
                      </span>
                    </label>
                    {(v.diskNote || v.diskResetLabel) && (
                      <div className="wz-disk-note">
                        {v.diskNote && <p className="wz-help">{v.diskNote}</p>}
                        {v.diskResetLabel && (
                          <button
                            type="button"
                            className="dw-btn dw-btn-quiet"
                            onClick={() => dispatch({ type: "reset-disk" })}
                          >
                            {v.diskResetLabel}
                          </button>
                        )}
                      </div>
                    )}
                  </div>
                )}
              {v.gpu && <GpuField row={v.gpu} dispatch={dispatch} onOpenExternal={onOpenExternal} />}
              {v.price && (
                <p className="wz-price" aria-label="Cost">
                  {v.price}
                </p>
              )}
              {v.resourceFacts.length > 0 && (
                <dl className="wz-summary" aria-label="Resources">
                  {v.resourceFacts.map((fact) => (
                    <Fragment key={fact.id}>
                      <dt>{fact.label}</dt>
                      <dd>
                        {fact.value}
                        {fact.symbol && (
                          <span className="wz-fact-warning" title={fact.help} aria-label={fact.help} role="img">
                            <Sym name={fact.symbol} size={12} />
                          </span>
                        )}
                      </dd>
                    </Fragment>
                  ))}
                </dl>
              )}
              {v.resourcesError && (
                <p className="dw-field-error" role="alert">
                  {v.resourcesError}
                </p>
              )}
            </div>
          )}

          {step === 2 && (
            <div className="wz-stack">
              <h2 className="wz-title">{v.title}</h2>
              <label className="dw-field">
                <span>{label(v, "name")}</span>
                <input
                  className="dw-input"
                  type="text"
                  value={v.name}
                  placeholder={field(v, "name")?.placeholder ?? undefined}
                  spellCheck={false}
                  aria-invalid={v.nameInvalid}
                  onChange={(event) => dispatch({ type: "set-name", name: event.target.value })}
                />
                {v.nameError && (
                  <span className="dw-field-error" role="alert">
                    {v.nameError}
                  </span>
                )}
              </label>
              <label className="dw-check">
                <input
                  type="checkbox"
                  checked={v.openWhenReady}
                  onChange={(event) => dispatch({ type: "set-open-when-ready", on: event.target.checked })}
                />
                <span>{label(v, "open-when-ready")}</span>
              </label>
              {v.streamNote && (
                <p className="wz-note" role="note">
                  {v.streamNote}
                </p>
              )}
            </div>
          )}

          {step === 3 && (
            <div className="wz-stack">
              <h2 className="wz-title">{v.title}</h2>
              <dl className="wz-summary" aria-label="Summary">
                {v.summary.map((fact) => (
                  <Fragment key={fact.label}>
                    <dt>{fact.label}</dt>
                    <dd>{fact.label === "Image" ? <code>{fact.value}</code> : fact.value}</dd>
                  </Fragment>
                ))}
              </dl>
            </div>
          )}
        </div>

        <footer className="wz-foot">
          <button type="button" className="dw-btn" onClick={cancel}>
            {v.labels.cancel}
          </button>
          <span className="wz-foot-spacer" />
          {v.showBack && (
            <button type="button" className="dw-btn" onClick={() => dispatch({ type: "back" })}>
              {v.labels.back}
            </button>
          )}
          {step < 3 ? (
            <button
              type="button"
              className="dw-btn dw-btn-primary"
              disabled={!v.canContinue}
              onClick={() => dispatch({ type: "next" })}
            >
              {v.primaryLabel}
            </button>
          ) : (
            <button
              type="button"
              className="dw-btn dw-btn-primary"
              onClick={() => {
                telemetry.recordSignals([{ type: "space-wizard", action: "submitted" }]);
                onCreate(v.plan);
              }}
            >
              {v.primaryLabel}
            </button>
          )}
        </footer>
      </div>
    </div>
  );
}

/** The GPU checkbox with a small "Learn more"; disabled with the reason
 * (tooltip and one quiet line) where this machine cannot. */
function GpuField({
  row,
  dispatch,
  onOpenExternal,
}: {
  row: GpuRow;
  dispatch: (action: WizardAction) => void;
  onOpenExternal?: (url: string) => void;
}) {
  const url = row.learnMoreUrl;
  return (
    <div className="wz-gpu">
      <div className="wz-gpu-row">
        <label className="dw-check" title={row.reason ?? undefined}>
          <input
            type="checkbox"
            checked={row.on}
            disabled={!row.enabled}
            onChange={(event) => dispatch({ type: "set-gpu", on: event.target.checked })}
          />
          <span>{row.label}</span>
        </label>
        {url && (
          <button type="button" className="dw-btn-link wz-gpu-learn" onClick={() => onOpenExternal?.(url)}>
            {row.learnMoreLabel}
          </button>
        )}
      </div>
      {!row.enabled && row.reason && <p className="wz-help">{row.reason}</p>}
    </div>
  );
}

function field(v: WizardView, id: string): WizardField | undefined {
  return v.fields.find((f) => f.id === id);
}

function label(v: WizardView, id: string): string {
  return field(v, id)?.label ?? "";
}

/** One field of the System step, drawn by its core `id`. */
function SystemField({
  field: f,
  view: v,
  dispatch,
  onConnectCloud,
}: {
  field: WizardField;
  view: WizardView;
  dispatch: (action: WizardAction) => void;
  onConnectCloud?: () => void;
}) {
  switch (f.id) {
    case "os":
      return (
        <div className="dw-field">
          <span className="dw-label">{f.label}</span>
          <div className="wz-tiles" role="group" aria-label={f.label}>
            {v.osTiles.map((tile) => (
              <button
                key={tile.id}
                type="button"
                className="wz-tile"
                aria-pressed={tile.pressed}
                disabled={!tile.enabled}
                onClick={() => dispatch({ type: "choose-os", os: tile.id })}
              >
                <span className="wz-tile-title">{tile.title}</span>
              </button>
            ))}
          </div>
        </div>
      );
    case "image":
      return <ImageField label={f.label} placeholder={f.placeholder} view={v.imageField} dispatch={dispatch} />;
    case "placement": {
      const chosen = v.placements.find((o) => o.selected);
      return (
        <>
          <label className="dw-field">
            <span>{f.label}</span>
            <select
              className="dw-select"
              aria-label={f.label}
              value={v.placementId}
              title={chosen?.detail || undefined}
              onChange={(event) => dispatch({ type: "choose-placement", on: event.target.value })}
            >
              {v.placements.map((o) => (
                <option key={o.id} value={o.id} disabled={!o.enabled} title={o.detail || undefined}>
                  {o.label}
                </option>
              ))}
            </select>
          </label>
          {f.error && (
            <p className="dw-error" role="alert">
              {f.error}
            </p>
          )}
        </>
      );
    }
    case "kind":
      return (
        <div className="dw-field">
          <span className="dw-label">{f.label}</span>
          <div className="wz-tiles" role="group" aria-label={f.label}>
            {v.kindTiles.map((tile) => (
              <button
                key={tile.id}
                type="button"
                className="wz-tile wz-tile-row"
                aria-pressed={tile.pressed}
                disabled={!tile.enabled}
                onClick={() => dispatch({ type: "choose-kind", kind: tile.id })}
              >
                <span className="wz-tile-title">{tile.title}</span>
              </button>
            ))}
          </div>
        </div>
      );
    case "runtime":
      return (
        <label className="dw-field">
          <span>{f.label}</span>
          <select
            className="dw-select"
            aria-label={f.label}
            value={v.runtime}
            disabled={!v.runtimeEnabled}
            onChange={(event) => dispatch({ type: "set-runtime", runtime: event.target.value })}
          >
            {v.runtimes.map((engine) => (
              <option key={engine.value} value={engine.value}>
                {engine.label}
              </option>
            ))}
          </select>
        </label>
      );
    case "connect-cloud":
      return onConnectCloud ? (
        <div>
          <button type="button" className="dw-btn-link" onClick={onConnectCloud}>
            {f.label}
          </button>
        </div>
      ) : null;
    case "connect-by-address":
      return (
        <div>
          <button type="button" className="dw-btn-link" onClick={() => dispatch({ type: "show-address" })}>
            {f.label}
          </button>
        </div>
      );
    default:
      return null;
  }
}

function AddByAddress({
  view,
  address,
  dispatch,
  onAdd,
  onDone,
}: {
  view: WizardView;
  address: { url: string; token: string; name: string };
  dispatch: (action: WizardAction) => void;
  onAdd: (url: string, token?: string, name?: string) => Promise<unknown>;
  onDone: () => void;
}) {
  const a = view.address;
  const submit = () => {
    const call = a.submit;
    if (!call) return;
    dispatch({ type: "submit-address" });
    onAdd(call.url, call.token ?? undefined, call.name ?? undefined)
      .then(() => onDone())
      .catch((err: unknown) =>
        dispatch({ type: "address-failed", error: err instanceof Error ? err.message : String(err) }),
      );
  };

  return (
    <div className="wz">
      <div className="wz-body">
        <div className="wz-stack">
          <h2 className="wz-title">{view.title}</h2>
          <label className="dw-field">
            <span>{label(view, "address")}</span>
            <input
              className="dw-input"
              type="text"
              value={address.url}
              placeholder={field(view, "address")?.placeholder ?? undefined}
              spellCheck={false}
              aria-invalid={a.showInvalid}
              onChange={(event) => dispatch({ type: "set-address", url: event.target.value })}
            />
          </label>
          <label className="dw-field">
            <span>{label(view, "token")}</span>
            <input
              className="dw-input"
              type="password"
              value={address.token}
              placeholder={field(view, "token")?.placeholder ?? undefined}
              spellCheck={false}
              onChange={(event) => dispatch({ type: "set-token", token: event.target.value })}
            />
          </label>
          <label className="dw-field">
            <span>{label(view, "address-name")}</span>
            <input
              className="dw-input"
              type="text"
              value={address.name}
              placeholder={field(view, "address-name")?.placeholder ?? undefined}
              onChange={(event) => dispatch({ type: "set-address-name", name: event.target.value })}
            />
          </label>
          {view.price && (
            <p className="wz-price" aria-label="Cost">
              {view.price}
            </p>
          )}
          {a.error && (
            <p className="dw-error" role="alert">
              {a.error}
            </p>
          )}
        </div>
      </div>
      <footer className="wz-foot">
        <button type="button" className="dw-btn" onClick={() => dispatch({ type: "hide-address" })}>
          {view.labels.back}
        </button>
        <span className="wz-foot-spacer" />
        <button type="button" className="dw-btn dw-btn-primary" disabled={!a.canSubmit} onClick={submit}>
          {a.submitLabel}
        </button>
      </footer>
    </div>
  );
}
