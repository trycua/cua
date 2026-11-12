// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { KeyboardEvent } from "react";

/** The app core's image field (`cua-spaces-app-core::wizard::ImageFieldView`). */
export interface ImageFieldView {
  text: string;
  open: boolean;
  groups: {
    id: string;
    label: string;
    rows: { ref: string; label: string; highlighted: boolean; selected: boolean }[];
  }[];
  error: string | null;
  custom: boolean;
}

type Action = { type: string } & Record<string, unknown>;

/**
 * An editable image reference with the catalog's presets as suggestions.
 * Filtering, the highlight, what Return picks and validation are the app
 * core's (the SwiftUI app renders the same view); this draws it.
 */
export function ImageField({
  label,
  placeholder,
  view,
  dispatch,
}: {
  label: string;
  placeholder?: string | null;
  view: ImageFieldView;
  dispatch: (action: Action) => void;
}) {
  const listId = "wz-image-suggestions";
  const rows = view.groups.flatMap((g) => g.rows);
  const active = rows.find((r) => r.highlighted);
  const optionId = (ref: string) => `wz-image-${ref.replace(/[^a-z0-9]/gi, "-")}`;

  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    switch (event.key) {
      case "ArrowDown":
      case "ArrowUp":
        event.preventDefault();
        dispatch({ type: "move-image-suggestion", delta: event.key === "ArrowDown" ? 1 : -1 });
        break;
      case "Enter":
        if (view.open) {
          event.preventDefault();
          dispatch({ type: "pick-image-suggestion" });
        }
        break;
      case "Escape":
        if (view.open) {
          event.preventDefault();
          event.stopPropagation();
          dispatch({ type: "dismiss-image-suggestions" });
        }
        break;
    }
  };

  return (
    <div className="dw-field wz-combo">
      <label className="dw-label" htmlFor="wz-image">
        {label}
      </label>
      <input
        id="wz-image"
        className="dw-input"
        type="text"
        role="combobox"
        aria-label={label}
        aria-expanded={view.open}
        aria-controls={listId}
        aria-autocomplete="list"
        aria-activedescendant={active ? optionId(active.ref) : undefined}
        aria-invalid={view.error != null}
        value={view.text}
        placeholder={placeholder ?? undefined}
        spellCheck={false}
        autoCapitalize="off"
        autoCorrect="off"
        onChange={(event) => dispatch({ type: "set-image-text", text: event.target.value })}
        onClick={() => {
          if (!view.open) dispatch({ type: "open-image-suggestions" });
        }}
        onKeyDown={onKeyDown}
        onBlur={() => dispatch({ type: "dismiss-image-suggestions" })}
      />
      {view.open && (
        <ul className="wz-combo-list" id={listId} role="listbox" aria-label={label}>
          {view.groups.map((group) => (
            <li key={group.id} role="presentation">
              <div className="wz-combo-group" role="presentation">
                {group.label}
              </div>
              <ul role="group" aria-label={group.label}>
                {group.rows.map((row) => (
                  <li
                    key={row.ref}
                    id={optionId(row.ref)}
                    role="option"
                    aria-selected={row.highlighted}
                    data-current={row.selected || undefined}
                    className="wz-combo-row"
                    // Pick before the input's blur closes the list.
                    onMouseDown={(event) => {
                      event.preventDefault();
                      dispatch({ type: "choose-image", ref: row.ref });
                    }}
                  >
                    <span className="wz-combo-ref">{row.ref}</span>
                    <span className="wz-combo-label">{row.label}</span>
                  </li>
                ))}
              </ul>
            </li>
          ))}
        </ul>
      )}
      {view.error && (
        <p className="dw-field-error" role="alert">
          {view.error}
        </p>
      )}
    </div>
  );
}
