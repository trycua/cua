// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { Space, SpaceAccess } from "@/bridge";
import { OsIcon } from "@/components/os-icon";
import { useT } from "@/i18n";
import { ownerLabel, peopleNames } from "@/lib/space-access";
import { avatarColor, initial, namesText } from "@/lib/spaces-page";
import { cn } from "@/lib/utils";

/**
 * A Space's mark: its OS for yours; the owner's (or team's) initial for one
 * shared with you or a team's. Dimmed while the Space is off.
 */
export function SpaceMark({ space, access, dim = false, className }: { space: Space; access: SpaceAccess; dim?: boolean; className?: string }) {
  const t = useT();
  const owner = ownerLabel(access);
  if (owner) {
    return (
      <span
        data-avatar={access.kind}
        title={access.kind === "team" ? owner : t("computers.tile.shared_by", { name: owner })}
        className={cn("flex size-6 shrink-0 items-center justify-center rounded-full text-[11px] font-semibold text-white", avatarColor(owner), className)}
      >
        {initial(owner)}
      </span>
    );
  }
  return (
    <span data-avatar="mine" className={cn("flex size-6 shrink-0 items-center justify-center rounded-full bg-tile text-muted-foreground", className)}>
      <OsIcon os={space.os} className={cn("size-3", dim && "opacity-50")} />
    </span>
  );
}

/** Who a Space of yours is shared with: up to three small avatars, then "+2". */
export function PeopleStack({ access, className }: { access: SpaceAccess; className?: string }) {
  const t = useT();
  if (access.kind !== "mine" || access.people.length === 0) return null;
  const names = peopleNames(access);
  const more = names.length - 3;
  return (
    <span
      data-people={names.length}
      title={t("computers.tile.shared_with", { names: namesText(names) })}
      className={cn("inline-flex shrink-0 items-center", className)}
    >
      {names.slice(0, 3).map((name, i) => (
        <span
          key={`${name}:${i}`}
          aria-hidden
          className={cn(
            "flex size-4 items-center justify-center rounded-full text-[9px] font-semibold text-white ring-2 ring-background",
            avatarColor(name),
            i > 0 && "-ml-1",
          )}
        >
          {initial(name)}
        </span>
      ))}
      {more > 0 ? <span className="ml-1 text-[11px] text-muted-foreground tabular-nums">+{more}</span> : null}
    </span>
  );
}
