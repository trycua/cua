// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useMemo } from "react";

import { useFirstSpaceOffers, useSpaces, type Space } from "@/bridge";
import { pickableSpaces, type NewKind } from "@/lib/ask-first";

/** What "Where it runs" offers: the kinds of new computer this Mac can make, and the running Spaces you have. */
export function useWhereOptions(): { kinds: NewKind[]; spaces: Space[] } {
  const { data } = useSpaces();
  const { offers } = useFirstSpaceOffers();
  const kinds = useMemo<NewKind[]>(() => {
    const k = offers.map((o) => ({ os: o.os, note: `Free. Ready in ${o.time}${o.size ? ` (${o.size})` : ""}.`, blocked: o.blocked }));
    return k.length ? k : [{ os: "linux", note: "Free. Ready in a few minutes.", blocked: null }];
  }, [offers]);
  const spaces = useMemo(() => pickableSpaces(data), [data]);
  return { kinds, spaces };
}
