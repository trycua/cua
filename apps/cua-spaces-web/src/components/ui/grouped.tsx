// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The app's layout vocabulary, taken from the Space page (CUA-2298):
 *
 * - SectionHeading: a small plain heading with whitespace above it. No rule.
 * - PlainList / PlainRow: rows with no lines between them; a row shows a
 *   surface on hover and when selected. For lists you pick from (machines,
 *   clouds, Spaces, vaults, notifications).
 * - GroupedSection / GroupedRow: ONE rounded container (8 px) holding rows
 *   that belong together (toggles, a machine's Spaces, a cloud's limits).
 *   Its separators run the full width of the container, so no line ever
 *   stops in mid-air. For settings and short lists you act on in place.
 * - GroupedAction: a button row at the end of a card ("Show all"). Text under
 *   or links after a card go in the section's `footer`, never a loose margin.
 * - SettingRow: label at the left, the control or value at the right, one
 *   line (an optional short help line under the label).
 * - FactRow: a fact (label, value) in a plain list, as the Space page's
 *   Details.
 * - DetailsDisclosure: an advanced part, folded by default.
 *
 * Rules the drift guard (`design-lint.test.ts`) holds every page to: one
 * radius (`rounded-lg`), no pills, no partial borders or `divide-*` outside
 * this file, no outlined boxes around content, no "a · b · c" chains.
 */

import { ChevronRightIcon } from "lucide-react";
import { useId, useState, type ComponentProps, type ReactNode } from "react";

import { cn } from "@/lib/utils";

type Div = ComponentProps<"div">;

/** "IN THIS SPACE", "DETAILS": the Space page's headings. */
export function SectionHeading({ children, action, className, ...props }: { children: ReactNode; action?: ReactNode } & Omit<ComponentProps<"h2">, "children">) {
  return (
    <div className={cn("mt-7 mb-1.5 flex min-h-5 items-center justify-between gap-3 px-1 first:mt-0", className)}>
      <h2 className="text-[11px] font-semibold tracking-wide text-muted-foreground uppercase" {...props}>
        {children}
      </h2>
      {action ? <div className="flex shrink-0 items-center gap-1 text-xs">{action}</div> : null}
    </div>
  );
}

/** A heading's quiet action ("Change limits…", "Show all"). */
export function HeadingAction({ className, ...props }: ComponentProps<"button">) {
  return (
    <button
      type="button"
      className={cn("cursor-default rounded-lg px-1.5 py-0.5 text-xs text-muted-foreground outline-none hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60", className)}
      {...props}
    />
  );
}

/**
 * One rounded container; its direct children are rows, and a separator the
 * full width of the container sits between each two.
 */
export function GroupedSection({
  title,
  action,
  footer,
  children,
  className,
  ...props
}: { title?: ReactNode; action?: ReactNode; footer?: ReactNode } & Omit<Div, "title">) {
  return (
    <section className={cn("min-w-0", className)}>
      {title ? <SectionHeading action={action}>{title}</SectionHeading> : null}
      <div data-grouped="" className="overflow-hidden rounded-lg border bg-card [&>*+*]:border-t" {...props}>
        {children}
      </div>
      {footer ? <div className="mt-1.5 px-1 text-xs text-muted-foreground">{footer}</div> : null}
    </section>
  );
}

/** A row that is a button, last in a card: "Show all", "Show fewer". */
export function GroupedAction({ className, ...props }: ComponentProps<"button">) {
  return (
    <button
      type="button"
      className={cn(
        "flex min-h-11 w-full cursor-default items-center gap-1.5 px-4 py-2.5 text-left text-[13px] text-muted-foreground outline-none hover:bg-accent/50 hover:text-foreground focus-visible:bg-accent/50",
        className,
      )}
      {...props}
    />
  );
}

/** A row in a GroupedSection: one line, 44 px at least. */
export function GroupedRow({ className, interactive, ...props }: Div & { interactive?: boolean }) {
  return <div className={cn("flex min-h-11 items-center gap-3 px-4 py-2.5 text-[13px]", interactive && "hover:bg-accent/50", className)} {...props} />;
}

/** Label at the left, control or value at the right. */
export function SettingRow({
  label,
  help,
  control,
  value,
  htmlFor,
  labelId,
  dim,
  className,
  ...props
}: {
  label: ReactNode;
  help?: ReactNode;
  control?: ReactNode;
  value?: ReactNode;
  htmlFor?: string;
  labelId?: string;
  dim?: boolean;
} & Omit<Div, "children">) {
  return (
    <GroupedRow className={cn("justify-between gap-6", className)} {...props}>
      <div className={cn("min-w-0", dim && "opacity-50")}>
        <label htmlFor={htmlFor} id={labelId} className="block text-[13px]">
          {label}
        </label>
        {help ? <p className="mt-px text-xs text-muted-foreground">{help}</p> : null}
      </div>
      {value !== undefined && value !== null ? <div className="min-w-0 shrink-0 truncate text-right text-[13px] text-muted-foreground select-text">{value}</div> : null}
      {control ? <div className="shrink-0">{control}</div> : null}
    </GroupedRow>
  );
}

/** A list you pick from: no lines between rows. */
export function PlainList({ className, ...props }: Div) {
  return <div className={cn("flex min-w-0 flex-col gap-px", className)} {...props} />;
}

const plainRow =
  "flex w-full min-w-0 cursor-default items-center gap-2.5 rounded-lg px-2 py-[7px] text-left text-[13px] outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring/60";

/** The surface a selected or hovered row shows (the sidebar's). */
export function plainRowClass(selected?: boolean, className?: string) {
  return cn(plainRow, selected ? "bg-row-selected" : "hover:bg-row-hover", className);
}

/** A row in a PlainList: a button by default. */
export function PlainRow({ selected, className, ...props }: { selected?: boolean } & ComponentProps<"button">) {
  return <button type="button" className={plainRowClass(selected, className)} {...props} />;
}

/** A fact in a plain list: label at the left, value at the right, no lines (the Space page's Details). */
export function FactRow({ label, value, className, ...props }: { label: ReactNode; value: ReactNode } & Omit<Div, "children">) {
  return (
    <div className={cn("flex h-8 items-center justify-between gap-4 px-1 text-[13px]", className)} {...props}>
      <span className="shrink-0 text-muted-foreground">{label}</span>
      <span className="min-w-0 truncate text-right select-text">{value}</span>
    </div>
  );
}

/** Advanced detail, folded by default: a heading-sized toggle and what it shows. */
export function DetailsDisclosure({
  title,
  children,
  defaultOpen = false,
  open: controlled,
  onOpenChange,
  className,
  ...props
}: {
  title: ReactNode;
  children: ReactNode;
  defaultOpen?: boolean;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
} & Omit<Div, "children" | "title">) {
  const [own, setOwn] = useState(defaultOpen);
  const open = controlled ?? own;
  const id = useId();
  return (
    <div className={cn("mt-7 min-w-0", className)} {...props}>
      <button
        type="button"
        aria-expanded={open}
        aria-controls={id}
        onClick={() => {
          setOwn(!open);
          onOpenChange?.(!open);
        }}
        className="-ml-1 flex cursor-default items-center gap-1 rounded-lg px-1 py-0.5 text-[11px] font-semibold tracking-wide text-muted-foreground uppercase outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
        data-details-toggle=""
      >
        <ChevronRightIcon className={cn("size-3 transition-transform", open && "rotate-90")} />
        {title}
      </button>
      {open ? (
        <div id={id} className="mt-1.5" data-details="">
          {children}
        </div>
      ) : null}
    </div>
  );
}
