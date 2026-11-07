// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useMemo } from "react";

import { useBridge, useFirstSpaceOffers, useMachines, useSpaces, type Space } from "@/bridge";
import { useHiddenSpaceIds } from "@/lib/prepare/store";
import { otherComputerDesktops, pickableSpaces, type NewKind } from "@/lib/ask-first";

/**
 * What "Where it runs" offers: the kinds of new computer this Mac can make, the running Spaces you have,
 * and your other computers that are online and share their desktop.
 */
export function useWhereOptions(): { kinds: NewKind[]; spaces: Space[]; computers: Space[] } {
  const { data } = useSpaces();
  const { data: machines } = useMachines();
  const { offers } = useFirstSpaceOffers();
  const kinds = useMemo<NewKind[]>(() => {
    const k = offers.map((o) => ({ os: o.os, note: `Free. Ready in ${o.time}${o.size ? ` (${o.size})` : ""}.`, blocked: o.blocked }));
    return k.length ? k : [{ os: "linux", note: "Free. Ready in a few minutes.", blocked: null }];
  }, [offers]);
  const { core } = useBridge();
  const warming = useHiddenSpaceIds(core);
  const spaces = useMemo(() => pickableSpaces(data?.filter((s) => !warming.has(s.id))), [data, warming]);
  const here = machines?.find((m) => m.current)?.id ?? null;
  const computers = useMemo(() => otherComputerDesktops(data, here), [data, here]);
  return { kinds, spaces, computers };
}
