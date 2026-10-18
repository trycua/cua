// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { ReactNode } from "react";

export function SettingsGroup({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="mb-7">
      <h2 className="mb-2 px-1 text-xs font-semibold text-muted-foreground">{title}</h2>
      <div className="divide-y overflow-hidden rounded-xl border bg-card shadow-xs">{children}</div>
    </section>
  );
}

export function SettingsRow({ label, description, control, htmlFor }: { label: string; description?: ReactNode; control: ReactNode; htmlFor?: string }) {
  return (
    <div className="flex min-h-12 items-center justify-between gap-6 px-4 py-2.5">
      <div className="min-w-0">
        <label htmlFor={htmlFor} className="block text-[13px]">
          {label}
        </label>
        {description ? <p className="mt-0.5 text-xs text-muted-foreground">{description}</p> : null}
      </div>
      <div className="shrink-0">{control}</div>
    </div>
  );
}
