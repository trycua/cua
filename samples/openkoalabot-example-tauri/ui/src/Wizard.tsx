// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The New Space wizard: a modal sheet in four steps (System, Resources,
// Options, Summary). Create calls `create_space`, which makes one real SDK
// call: `Spaces::create` on this machine or in Cua Cloud.
import { useState } from "react"
import { imageByRef, imageGroups } from "./images"
import { Alert, Cloud, Info, Laptop, OsGlyph } from "./icons"
import {
  CPU_RANGE,
  MEMORY_RANGE,
  OSES,
  OS_LABEL,
  RUNTIME_LABEL,
  STEPS,
  back,
  blocker,
  formatMemory,
  initialState,
  next,
  osAvailable,
  pickImage,
  pickOs,
  pickTarget,
  summaryRows,
  supports,
  toPlan,
  type SpacePlan,
  type WizardState,
} from "./spaceWizard"

/** The image dropdown: exactly the published entries of the shared list,
 *  one `<optgroup>` per group, in file order. */
export function ImageSelect({ value, onChange, id }: { value: string; onChange: (ref: string) => void; id?: string }) {
  return (
    <select id={id} className="select" value={value} onChange={(e) => onChange(e.target.value)}>
      {imageGroups().map((g) => (
        <optgroup key={g.id} label={g.label}>
          {g.images.map((i) => (
            <option key={i.ref} value={i.ref}>
              {i.name} ({i.ref})
            </option>
          ))}
        </optgroup>
      ))}
    </select>
  )
}

export function Wizard(props: {
  cloud: boolean
  onCancel: () => void
  onCreate: (plan: SpacePlan, openWhenReady: boolean) => Promise<boolean>
  onAddByAddress: () => void
  /** Where to open (walkthroughs); defaults to step 1. */
  initial?: Partial<WizardState>
}) {
  const [s, setS] = useState<WizardState>(() => ({ ...initialState(props.cloud), ...props.initial }))
  const [busy, setBusy] = useState(false)
  const image = imageByRef(s.image)
  const why = blocker(s)
  const last = s.step === STEPS.length - 1

  const create = async () => {
    setBusy(true)
    const ok = await props.onCreate(toPlan(s), s.openWhenReady)
    setBusy(false)
    if (ok) props.onCancel()
  }

  return (
    <div className="scrim" role="dialog" aria-modal="true" aria-label="New Space">
      <div className="sheet">
        <div className="sheet-head">
          <h2>New Space</h2>
          <p>A computer for your Bots: pick a system, where it runs, and a name.</p>
        </div>
        <div className="steps" aria-label="Steps">
          {STEPS.map((label, i) => (
            <div key={label} style={{ display: "contents" }}>
              {i > 0 && <div className="step-line" />}
              <div className={`step ${i === s.step ? "on" : i < s.step ? "done" : ""}`}>
                <span className="n">{i + 1}</span>
                {label}
              </div>
            </div>
          ))}
        </div>
        <div className="sheet-body">
          {s.step === 0 && image && (
            <>
              <div className="field">
                <span>Operating system</span>
                <div className="tiles">
                  {OSES.map((os) => (
                    <button key={os} className={`tile ${s.os === os ? "on" : ""}`} disabled={!osAvailable(os)} onClick={() => setS(pickOs(s, os))}>
                      <OsGlyph os={os} />
                      {OS_LABEL[os]}
                    </button>
                  ))}
                </div>
              </div>
              <label className="field">
                <span>Image</span>
                <ImageSelect value={s.image} onChange={(ref) => setS(pickImage(s, ref))} />
                <span className="image-summary">{image.summary}</span>
              </label>
              <div className="field">
                <span>Where it runs</span>
                <div className="seg">
                  <button className={`tile ${s.target === "cloud" ? "on" : ""}`} disabled={!supports(image, "cloud")} onClick={() => setS(pickTarget(s, "cloud"))}>
                    <Cloud />
                    Cua Cloud
                    <small>{image.cloud ? RUNTIME_LABEL[image.cloud] : "Not available for this image"}</small>
                  </button>
                  <button className={`tile ${s.target === "local" ? "on" : ""}`} disabled={!supports(image, "local")} onClick={() => setS(pickTarget(s, "local"))}>
                    <Laptop />
                    This machine
                    <small>{image.local ? RUNTIME_LABEL[image.local] : "Not available for this image"}</small>
                  </button>
                </div>
              </div>
            </>
          )}
          {s.step === 1 && image && s.target === "local" && (
            <>
              <div className="slider">
                <div className="top">
                  <span>CPU cores</span>
                  <output>{s.cpus}</output>
                </div>
                <input type="range" min={CPU_RANGE.min} max={CPU_RANGE.max} value={s.cpus} onChange={(e) => setS({ ...s, cpus: Number(e.target.value) })} />
              </div>
              <div className="slider">
                <div className="top">
                  <span>Memory</span>
                  <output>{formatMemory(s.memoryMb)}</output>
                </div>
                <input type="range" min={MEMORY_RANGE.min} max={MEMORY_RANGE.max} step={MEMORY_RANGE.step} value={s.memoryMb} onChange={(e) => setS({ ...s, memoryMb: Number(e.target.value) })} />
              </div>
              <div className="note">
                <Info />
                <span>
                  Runtime: {RUNTIME_LABEL[image.local ?? ""]}. The SDK sets it up on this machine itself; the first start downloads the image.
                </span>
              </div>
            </>
          )}
          {s.step === 1 && image && s.target === "cloud" && (
            <>
              <div className="note">
                <Cloud />
                <span>
                  {image.cloud === "gvisor"
                    ? "This image runs in a gVisor sandbox: a container with its own kernel boundary. It starts in seconds and uses little memory."
                    : "This image runs as a KubeVirt virtual machine: a full VM with its own kernel. It takes longer to start and suits desktops that need one."}
                </span>
              </div>
              <div className="note">
                <Info />
                <span>A Cua Cloud Space is metered until you delete it.</span>
              </div>
              {!props.cloud && (
                <div className="note warn">
                  <Alert />
                  <span>Cua Cloud is not signed in. Run `cua auth login` (or set CUA_CLIENT_ID and CUA_CLIENT_SECRET) and restart the app.</span>
                </div>
              )}
            </>
          )}
          {s.step === 2 && image && (
            <>
              <label className="field">
                <span>Name</span>
                <input className={`input ${s.name && !/^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/.test(s.name) ? "invalid" : ""}`} value={s.name} spellCheck={false} onChange={(e) => setS({ ...s, name: e.target.value.toLowerCase() })} />
                <span className="hint">A DNS label: lowercase letters, digits and dashes.</span>
              </label>
              <label className="check">
                <input type="checkbox" checked={s.openWhenReady} onChange={(e) => setS({ ...s, openWhenReady: e.target.checked })} />
                <span>
                  Open the desktop when ready
                  <div className="hint">Shows the Space in the Computer panel as soon as it answers.</div>
                </span>
              </label>
              {!image.spacesd && (
                <div className="note warn">
                  <Alert />
                  <span>{image.name} does not run cua-spacesd yet: Bots, file drop, streaming and teleport need it. The Space still starts and shows up in the list.</span>
                </div>
              )}
            </>
          )}
          {s.step === 3 && (
            <dl className="summary">
              {summaryRows(s).map(([k, v]) => (
                <div key={k} style={{ display: "contents" }}>
                  <dt>{k}</dt>
                  <dd>{v}</dd>
                </div>
              ))}
            </dl>
          )}
          {why && <div className="error-text">{why}</div>}
        </div>
        <div className="sheet-foot">
          <button className="btn btn-ghost" onClick={props.onAddByAddress}>
            Add by address…
          </button>
          <div className="spacer" />
          <button className="btn" onClick={props.onCancel}>
            Cancel
          </button>
          {s.step > 0 && (
            <button className="btn" onClick={() => setS(back(s))}>
              Back
            </button>
          )}
          {last ? (
            <button className="btn btn-primary" disabled={!!why || busy} onClick={create}>
              {busy ? "Creating…" : "Create"}
            </button>
          ) : (
            <button className="btn btn-primary" disabled={!!why} onClick={() => setS(next(s))}>
              Continue
            </button>
          )}
        </div>
      </div>
    </div>
  )
}
