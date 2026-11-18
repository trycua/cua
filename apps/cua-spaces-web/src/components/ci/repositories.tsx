// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { SearchIcon } from "lucide-react";

import type { CiAction, CiPickerView } from "@/bridge";
import { Button } from "@/components/ui/button";
import { GroupedRow, GroupedSection, HeadingAction, SectionHeading } from "@/components/ui/grouped";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { cn } from "@/lib/utils";

/**
 * Repositories: how many can run jobs, with a quiet "Choose" that opens the
 * picker. The picker searches, shows each repository's status and its jobs
 * this week, switches one on or off (the hosted app's list), and sends the
 * person to GitHub to choose which repositories the app can see.
 */
export function Repositories({
  picker,
  open,
  onAction,
  onSwitch,
  onOpen,
}: {
  picker: CiPickerView;
  open: boolean;
  onAction(a: CiAction): void;
  onSwitch(name: string, allowed: boolean): void;
  onOpen(url: string): void;
}) {
  return (
    <section data-ci-repositories="" className="min-w-0">
      <SectionHeading
        className="mt-0"
        action={
          <HeadingAction data-ci-picker-toggle="" onClick={() => onAction({ type: "picker", open: !open })}>
            {open ? "Hide" : "Choose"}
          </HeadingAction>
        }
      >
        {picker.title}
      </SectionHeading>
      <p className="mb-1.5 px-1 text-[13px] text-muted-foreground" data-ci-repo-summary="">
        {picker.summary}
      </p>
      {open ? (
        <>
          <div className="relative mb-2">
            <SearchIcon className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground" aria-hidden />
            <Input
              aria-label={picker.searchLabel}
              placeholder={picker.searchLabel}
              className="pl-8"
              data-ci-repo-search=""
              onChange={(e) => onAction({ type: "query", value: e.currentTarget.value })}
            />
          </div>
          <GroupedSection>
            {picker.rows.length > 0 ? (
              picker.rows.map((r) => (
                <GroupedRow key={r.name} data-ci-repo={r.name} data-allowed={r.allowed ? "true" : "false"} className="justify-between">
                  <div className="min-w-0">
                    <p className="truncate text-[13px]">{r.name}</p>
                    <p className="text-xs text-muted-foreground">{r.jobs}</p>
                  </div>
                  <div className="flex shrink-0 items-center gap-3">
                    <span className={cn("text-xs", r.allowed ? "text-success" : "text-muted-foreground")} data-ci-repo-status="">
                      {r.status}
                    </span>
                    {r.canToggle ? <Switch aria-label={r.name} checked={r.allowed} onCheckedChange={(on) => onSwitch(r.name, on)} /> : null}
                  </div>
                </GroupedRow>
              ))
            ) : (
              <p className="px-4 py-6 text-center text-[13px] text-muted-foreground">{picker.empty}</p>
            )}
          </GroupedSection>
          <div className="mt-2 flex items-center gap-2">
            {picker.chooseUrl ? (
              <Button size="sm" variant="outline" data-ci-choose-github="" onClick={() => onOpen(picker.chooseUrl!)}>
                {picker.chooseLabel}
              </Button>
            ) : null}
            <Button size="sm" variant="outline" data-ci-connect-account="" onClick={() => onAction({ type: "connectOpen" })}>
              {picker.connectLabel}
            </Button>
          </div>
        </>
      ) : null}
    </section>
  );
}
