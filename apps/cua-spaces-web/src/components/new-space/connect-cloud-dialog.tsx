// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { CheckIcon } from "lucide-react";

import { useConnectCloud, type CloudConnectAction, type CloudField } from "@/bridge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";

/**
 * "Connect a cloud": one row per provider (a check when this machine has
 * its sign-in) and "No cloud", the region, project or environment, what
 * the provider will touch, Test (creates nothing) and Connect. The app core
 * decides every word and request (`cloudConnect.*`); this draws it.
 */
export function ConnectCloudDialog() {
  const connect = useConnectCloud();
  const v = connect.view;
  const send = (action: CloudConnectAction) => void connect.send(action);

  const text = (f: CloudField, type: "set-value" | "set-profile") => (
    <div data-cloud-field={f.id} className="px-4 py-2.5">
      <label htmlFor={`cc-${f.id}`} className="mb-2 block text-[13px]">
        {f.label}
      </label>
      <Input
        id={`cc-${f.id}`}
        value={f.value}
        placeholder={f.placeholder}
        spellCheck={false}
        autoComplete="off"
        onChange={(e) => send({ type, text: e.target.value })}
      />
    </div>
  );

  return (
    <Dialog open={connect.open && v !== null} onOpenChange={(open) => !open && connect.close()}>
      <DialogPopup className="top-[7vh] w-[min(560px,calc(100vw-2rem))]" data-connect-cloud="">
        {v ? (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              if (v.canConnect) send({ type: "connect" });
            }}
          >
            <div className="max-h-[78vh] space-y-4 overflow-y-auto px-6 pt-5 pb-4">
              <DialogTitle className="text-[15px] font-semibold" data-cloud-title="">
                {v.title}
              </DialogTitle>

              <div role="radiogroup" aria-label="Clouds" className="divide-y overflow-hidden rounded-xl border bg-card shadow-xs">
                {v.rows.map((r) => (
                  <button
                    key={r.id}
                    type="button"
                    role="radio"
                    aria-checked={r.selected}
                    data-cloud-row={r.id}
                    onClick={() => send({ type: "select", name: r.id })}
                    className="flex w-full items-center gap-3 px-4 py-2.5 text-left outline-none hover:bg-foreground/[0.03] focus-visible:bg-foreground/[0.05]"
                  >
                    <span
                      className={cn(
                        "flex size-4 shrink-0 items-center justify-center rounded-full border border-input bg-card shadow-xs dark:bg-input/30",
                        r.selected && "border-brand bg-brand dark:bg-brand",
                      )}
                    >
                      {r.selected ? <span className="size-1.5 rounded-full bg-white" /> : null}
                    </span>
                    <span className="min-w-0 flex-1">
                      <span className="block text-[13px]" data-cloud-row-title="">
                        {r.title}
                      </span>
                      <span className="block truncate text-xs text-muted-foreground" data-cloud-row-detail="">
                        {r.detail}
                      </span>
                    </span>
                    {r.found ? (
                      <span role="img" aria-label="Sign-in found" title="Sign-in found" data-cloud-found="">
                        <CheckIcon className="size-4 text-success" />
                      </span>
                    ) : null}
                  </button>
                ))}
              </div>

              {v.field || v.profileField ? (
                <div className="divide-y overflow-hidden rounded-xl border bg-card shadow-xs">
                  {v.field ? text(v.field, "set-value") : null}
                  {v.profileField ? text(v.profileField, "set-profile") : null}
                  {v.field ? (
                    <label className="flex items-center gap-2 px-4 py-2.5 text-[13px]">
                      <Checkbox data-make-default="" checked={v.makeDefault} onCheckedChange={(on) => send({ type: "set-make-default", on })} />
                      {v.makeDefaultLabel}
                    </label>
                  ) : null}
                </div>
              ) : null}

              {v.touches.length ? (
                <div aria-label="What Cua will touch" data-cloud-touches="" className="space-y-1.5 px-1 text-xs text-muted-foreground">
                  {v.touches.map((line) => (
                    <p key={line}>{line}</p>
                  ))}
                </div>
              ) : null}

              {v.checks.length ? (
                <ul aria-label="Checks" className="space-y-1 px-1">
                  {v.checks.map((c) => (
                    <li key={c.text} data-cloud-check={c.ok ? "ok" : "failed"} className="flex items-center gap-2 text-[13px]">
                      <span className={cn("size-1.5 shrink-0 rounded-full", c.ok ? "bg-success" : "bg-destructive")} aria-hidden="true" />
                      {c.text}
                    </li>
                  ))}
                </ul>
              ) : null}
              {v.result ? (
                <p data-cloud-result="" className="px-1 text-[13px] text-muted-foreground">
                  {v.result}
                </p>
              ) : null}
              {v.error ? (
                <p role="alert" data-cloud-error="" className="px-1 text-xs text-destructive">
                  {v.error}
                </p>
              ) : null}
            </div>

            <div className="flex items-center gap-2 border-t px-6 py-3">
              <Button variant="outline" data-cloud-cancel="" onClick={connect.close}>
                {v.cancelLabel}
              </Button>
              <span className="flex-1" />
              <Button variant="outline" data-cloud-test="" title={v.testHelp} disabled={!v.canTest} onClick={() => send({ type: "test" })}>
                {v.testLabel}
              </Button>
              <Button type="submit" data-cloud-connect="" disabled={!v.canConnect}>
                {v.connectLabel}
              </Button>
            </div>
          </form>
        ) : null}
      </DialogPopup>
    </Dialog>
  );
}
