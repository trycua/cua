// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Link } from "@tanstack/react-router";

import type { Machine, ResourcesSnapshot } from "@/bridge";
import { computerWarnings } from "@/components/resources/computer-resources";
import { useT } from "@/i18n";
import { THIS_COMPUTER } from "@/lib/machines";

/** Home's warning lines above your computers: one quiet row each, only while there is one, on a soft fill in the one radius (no outline). */
export function ComputerWarnings({ snapshot, machines }: { snapshot: ResourcesSnapshot | null; machines: readonly Machine[] }) {
  const t = useT();
  const warnings = computerWarnings(snapshot, machines);
  if (!warnings.length) return null;
  return (
    <section data-computer-warnings="" aria-label={t("home.pool.warnings_label")} className="mb-5 flex flex-col gap-1.5">
      {warnings.map((w) => (
        <p
          key={`${w.kind}:${w.machineId}`}
          role="status"
          data-computer-warning={w.kind}
          data-low-disk={w.level}
          className="flex min-h-9 items-center gap-2.5 rounded-lg bg-tile px-3 py-2 text-[13px] text-muted-foreground"
        >
          <span
            aria-hidden
            className={w.level === "critical" ? "size-1.5 shrink-0 rounded-full bg-destructive" : "size-1.5 shrink-0 rounded-full bg-[#d18a12]"}
          />
          <span className="min-w-0 flex-1">{w.text}</span>
          {w.local ? (
            // This computer's page: the Spaces made on it, each with what it takes on the disk.
            <Link
              to="/spaces/computer/$computerId"
              params={{ computerId: THIS_COMPUTER }}
              data-see-space-use=""
              className="shrink-0 text-[13px] font-medium text-foreground underline-offset-2 hover:underline"
            >
              {t("home.pool.see_space_use")}
            </Link>
          ) : null}
        </p>
      ))}
    </section>
  );
}
