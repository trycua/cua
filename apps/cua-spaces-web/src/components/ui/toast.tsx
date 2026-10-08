// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Toast } from "@base-ui/react/toast";
import { CircleAlertIcon, CircleCheckIcon, InfoIcon, XIcon } from "lucide-react";
import type { ReactNode } from "react";

export const toastManager = Toast.createToastManager();

type ToastType = "info" | "success" | "error";

/** How long an info or success toast stays. Errors stay until dismissed. */
export const TOAST_MS = 5000;

export function toast(title: string, options: { description?: string; type?: ToastType } = {}) {
  const type = options.type ?? "info";
  // Base UI's own timer pauses while the window is unfocused and resumes on
  // focus, which a webview in a native window may never report (an
  // "Opening…" toast stayed 10+ min). A plain timer closes it either way.
  const id = toastManager.add({ title, description: options.description, type, timeout: 0 });
  if (type !== "error") setTimeout(() => toastManager.close(id), TOAST_MS);
  return id;
}

/** A failed action: `title` says what didn't happen, the error says why. */
export const toastError = (title: string) => (e: unknown) =>
  toast(title, { description: e instanceof Error ? e.message : String(e), type: "error" });

const ICONS = { info: InfoIcon, success: CircleCheckIcon, error: CircleAlertIcon } as const;

export function ToastProvider({ children }: { children: ReactNode }) {
  return (
    <Toast.Provider toastManager={toastManager} limit={3}>
      {children}
      <Toast.Portal>
        <Toast.Viewport className="fixed right-4 bottom-4 z-[60] flex w-80 flex-col-reverse gap-2 outline-none">
          <ToastList />
        </Toast.Viewport>
      </Toast.Portal>
    </Toast.Provider>
  );
}

function ToastList() {
  const { toasts } = Toast.useToastManager();
  return toasts.map((t) => {
    const Icon = ICONS[(t.type as ToastType | undefined) ?? "info"] ?? InfoIcon;
    return (
      <Toast.Root
        key={t.id}
        toast={t}
        data-video-occluder=""
        className="glass flex items-start gap-2.5 rounded-xl border p-3 pr-2 shadow-float transition-[opacity,translate] duration-200 ease-out-soft data-ending-style:translate-y-2 data-ending-style:opacity-0 data-starting-style:translate-y-2 data-starting-style:opacity-0"
      >
        <Icon className={t.type === "error" ? "mt-px size-4 text-destructive" : t.type === "success" ? "mt-px size-4 text-success" : "mt-px size-4 text-brand-strong"} />
        <Toast.Content className="min-w-0 flex-1">
          <Toast.Title className="text-[13px] font-medium" />
          <Toast.Description className="text-xs text-muted-foreground" />
        </Toast.Content>
        <Toast.Close aria-label="Dismiss" className="rounded-md p-0.5 text-muted-foreground hover:bg-accent hover:text-foreground">
          <XIcon className="size-3.5" />
        </Toast.Close>
      </Toast.Root>
    );
  });
}
