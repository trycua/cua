// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { ReactNode } from "react";

import { GroupedSection, SettingRow } from "@/components/ui/grouped";
import { cn } from "@/lib/utils";

/**
 * A titled grouped container of settings (components/ui/grouped.tsx). Every
 * Settings tab is made of these, so the gap between sections (32 px), the
 * heading style and the footer under a card are the same on each.
 */
export function SettingsGroup({
  title,
  footer,
  action,
  children,
  className,
  ...props
}: { title: ReactNode; footer?: ReactNode; action?: ReactNode; children: ReactNode; className?: string } & Record<`data-${string}`, unknown>) {
  return (
    <GroupedSection title={title} footer={footer} action={action} className={cn("mb-8", className)} {...props}>
      {children}
    </GroupedSection>
  );
}

export function SettingsRow({ label, description, control, htmlFor }: { label: ReactNode; description?: ReactNode; control: ReactNode; htmlFor?: string }) {
  return <SettingRow label={label} help={description} control={control} htmlFor={htmlFor} />;
}
