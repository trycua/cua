// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Popover as PopoverPrimitive } from "@base-ui/react/popover";
import { useNavigate } from "@tanstack/react-router";
import { CheckIcon, ChevronRightIcon } from "lucide-react";
import { useState } from "react";

import { pointerChange, pointerSettings, useDriver, type DriverHook, type DriverStatus } from "@/bridge";
import { AppMarkImage } from "@/components/agents/harness-mark";
import { CuaKey } from "@/components/cua-key";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { toast, toastError } from "@/components/ui/toast";
import { topBarText } from "./copy";

/**
 * The top bar's "Agents can use this Mac" (CUA-2706, variation E): after the
 * first run, the driver stays named on every page. A click opens a small
 * panel: who is using which app (with Pause), the pointer switch, Update
 * when one is ready, and the way to the Cua Driver page.
 */
export function TopBarDriver() {
  const driver = useDriver();
  const [open, setOpen] = useState(false);
  const s = driver.status;
  if (!driver.available || !s) return null;
  const ok = s.state === "ready";
  return (
    <PopoverPrimitive.Root open={open} onOpenChange={setOpen}>
      <PopoverPrimitive.Trigger
        render={
          <Button variant="ghost" size="sm" className="h-7 gap-1.5 px-2 text-xs font-normal text-foreground/85" data-top-bar-driver={s.state}>
            {ok ? (
              <span className="flex size-3.5 items-center justify-center rounded-full bg-[#1f9d55] text-white" aria-hidden>
                <CheckIcon className="size-2.5" strokeWidth={3} />
              </span>
            ) : (
              <CuaKey size={16} />
            )}
            {topBarText(s)}
          </Button>
        }
      />
      <PopoverPrimitive.Portal>
        <PopoverPrimitive.Positioner side="bottom" align="end" sideOffset={6} className="z-50 outline-none">
          <PopoverPrimitive.Popup
            data-driver-panel=""
            /* design-ok: a floating panel's edge, like the update popover */
            className="glass w-80 origin-(--transform-origin) rounded-lg border text-popover-foreground shadow-float outline-none transition-[opacity,scale] data-ending-style:scale-95 data-ending-style:opacity-0 data-starting-style:scale-95 data-starting-style:opacity-0"
          >
            <DriverPanel driver={driver} status={s} close={() => setOpen(false)} />
          </PopoverPrimitive.Popup>
        </PopoverPrimitive.Positioner>
      </PopoverPrimitive.Portal>
    </PopoverPrimitive.Root>
  );
}

function DriverPanel({ driver, status, close }: { driver: DriverHook; status: DriverStatus; close: () => void }) {
  const navigate = useNavigate();
  const [updating, setUpdating] = useState(false);
  const show = pointerSettings(driver.config).show;
  const summary = status.version ? `${topBarText(status)}, version ${status.version}` : topBarText(status);
  const open = () => {
    close();
    void navigate({ to: "/driver" });
  };
  return (
    // design-ok: full-width separators between the panel's rows, as GroupedSection draws them
    <div className="flex flex-col [&>*+*]:border-t">
      <div className="flex items-center gap-3 px-3.5 py-3">
        <CuaKey size={28} />
        <div className="min-w-0">
          <PopoverPrimitive.Title className="text-[13px] font-semibold">Cua Driver</PopoverPrimitive.Title>
          <PopoverPrimitive.Description className="truncate text-xs text-muted-foreground">{summary}</PopoverPrimitive.Description>
        </div>
      </div>
      {status.activity.map((a) => (
        <div key={`${a.app}:${a.target}`} className="flex items-center gap-2.5 px-3.5 py-2.5 text-[13px]" data-driver-activity={a.app}>
          <AppMarkImage agent={a.app === "Claude Code" ? "claude-code" : a.app === "Codex" ? "codex" : "t3code"} className="size-4" />
          <span className="min-w-0 flex-1 truncate">
            {a.app} is using {a.target}
          </span>
          {status.canPause ? (
            <Button variant="outline" size="sm" onClick={() => void driver.pause(true).catch(toastError("Couldn't pause agents"))}>
              Pause
            </Button>
          ) : null}
        </div>
      ))}
      {status.state === "paused" && status.canPause ? (
        <div className="flex items-center gap-2.5 px-3.5 py-2.5 text-[13px]">
          <span className="flex-1">Agents are paused</span>
          <Button variant="outline" size="sm" onClick={() => void driver.pause(false).catch(toastError("Couldn't resume agents"))}>
            Resume
          </Button>
        </div>
      ) : null}
      {status.state === "needs-permission" ? (
        <div className="flex items-center gap-2.5 px-3.5 py-2.5 text-[13px]">
          <span className="flex-1">Allow Cua Spaces in System Settings</span>
          <Button size="sm" onClick={open}>
            Set up
          </Button>
        </div>
      ) : null}
      {show !== null ? (
        <label className="flex items-center gap-2.5 px-3.5 py-2.5 text-[13px]">
          <span className="flex-1">Show agent pointer</span>
          <Switch checked={show} onCheckedChange={(on) => void driver.setConfig(pointerChange("show", on)).catch(toastError("Couldn't change the pointer"))} />
        </label>
      ) : null}
      {status.update ? (
        <div className="flex items-center gap-2.5 px-3.5 py-2.5 text-[13px]" data-driver-panel-update="">
          <div className="min-w-0 flex-1">
            <div>Version {status.update.version} is ready</div>
            <div className="text-xs text-muted-foreground">About a minute, no new permission</div>
          </div>
          <Button
            size="sm"
            disabled={updating}
            onClick={() => {
              setUpdating(true);
              const to = status.update?.version;
              void driver
                .update()
                .then(() => toast(`Cua Driver ${to} is installed`))
                .catch(toastError("Couldn't update Cua Driver"))
                .finally(() => setUpdating(false));
            }}
          >
            {updating ? "Updating…" : "Update"}
          </Button>
        </div>
      ) : null}
      <button type="button" onClick={open} className="flex items-center px-3.5 py-2.5 text-left text-[13px] outline-none hover:bg-accent/50 focus-visible:bg-accent/50" data-driver-panel-open="">
        <span className="flex-1">Cua Driver settings</span>
        <ChevronRightIcon className="size-3.5 text-muted-foreground" />
      </button>
    </div>
  );
}
