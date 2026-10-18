// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { KeyboardEvent } from "react";

/** Return presses the page's primary button, unless a control that takes
 * Return itself (a button, a field, a link, a switch) has focus. */
export function pressPrimaryOnReturn(e: KeyboardEvent<HTMLDivElement>) {
  if (e.key !== "Enter" || e.defaultPrevented || e.nativeEvent.isComposing || e.metaKey || e.ctrlKey || e.altKey || e.shiftKey) return;
  const target = e.target as HTMLElement;
  if (target.closest("button, a, input, textarea, select, [role=switch], [role=checkbox], [contenteditable=true], form")) return;
  const primary = e.currentTarget.querySelector<HTMLButtonElement>("[data-onboarding-primary]:not(:disabled)");
  if (!primary) return;
  e.preventDefault();
  primary.click();
}
