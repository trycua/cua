// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { KeyboardEvent } from "react";

import type { ImageFieldView, WizardAction } from "@/bridge";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";

const optionId = (ref: string) => `ns-image-${ref.replace(/[^a-z0-9]/gi, "-")}`;

/**
 * The image: any reference, with the catalog's presets as suggestions.
 * Filtering, the highlight, what Return picks and the check on a typed
 * reference are the app core's; this draws them.
 */
export function ImageField({
  label,
  placeholder,
  view,
  send,
}: {
  label: string;
  placeholder?: string | null;
  view: ImageFieldView;
  send: (action: WizardAction) => void;
}) {
  const active = view.groups.flatMap((g) => g.rows).find((r) => r.highlighted);

  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    switch (event.key) {
      case "ArrowDown":
      case "ArrowUp":
        event.preventDefault();
        send({ type: "move-image-suggestion", delta: event.key === "ArrowDown" ? 1 : -1 });
        break;
      case "Enter":
        if (view.open) {
          event.preventDefault();
          send({ type: "pick-image-suggestion" });
        }
        break;
      case "Escape":
        if (view.open) {
          event.preventDefault();
          event.stopPropagation();
          send({ type: "dismiss-image-suggestions" });
        }
        break;
    }
  };

  return (
    <div className="relative">
      <Input
        id="ns-image"
        data-image-input=""
        role="combobox"
        aria-label={label}
        aria-expanded={view.open}
        aria-controls="ns-image-suggestions"
        aria-autocomplete="list"
        aria-activedescendant={active ? optionId(active.ref) : undefined}
        aria-invalid={view.error != null}
        value={view.text}
        placeholder={placeholder ?? undefined}
        spellCheck={false}
        autoCapitalize="off"
        autoCorrect="off"
        autoComplete="off"
        className="font-mono text-xs aria-invalid:border-destructive"
        onChange={(event) => send({ type: "set-image-text", text: event.target.value })}
        onClick={() => {
          if (!view.open) send({ type: "open-image-suggestions" });
        }}
        onKeyDown={onKeyDown}
        onBlur={() => {
          if (view.open) send({ type: "dismiss-image-suggestions" });
        }}
      />
      {view.open ? (
        <div
          id="ns-image-suggestions"
          role="listbox"
          aria-label={label}
          data-image-suggestions=""
          className="absolute inset-x-0 top-full z-10 mt-1 bg-popover text-popover-foreground max-h-64 overflow-y-auto rounded-lg border p-1 shadow-float"
        >
          {view.groups.map((group) => (
            <div key={group.id} role="group" aria-label={group.label}>
              <div className="px-2 pt-1.5 pb-1 text-2xs font-medium text-muted-foreground">{group.label}</div>
              {group.rows.map((row) => (
                <div
                  key={row.ref}
                  id={optionId(row.ref)}
                  role="option"
                  aria-selected={row.highlighted}
                  data-image-row={row.ref}
                  data-current={row.selected || undefined}
                  className={cn(
                    "flex cursor-default items-baseline justify-between gap-3 rounded-md px-2 py-1 text-[13px]",
                    row.highlighted ? "bg-brand text-white" : "hover:bg-foreground/[0.06]",
                  )}
                  // Pick before the field's blur closes the list.
                  onMouseDown={(event) => {
                    event.preventDefault();
                    send({ type: "choose-image", ref: row.ref });
                  }}
                >
                  <span className="truncate font-mono text-xs">{row.ref}</span>
                  <span className={cn("shrink-0 text-xs", row.highlighted ? "text-white/80" : "text-muted-foreground")}>{row.label}</span>
                </div>
              ))}
            </div>
          ))}
        </div>
      ) : null}
      {view.error ? (
        <p role="alert" data-field-error="image" className="mt-1.5 text-xs text-destructive">
          {view.error}
        </p>
      ) : null}
    </div>
  );
}
