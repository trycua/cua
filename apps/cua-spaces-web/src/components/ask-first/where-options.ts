// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useMemo } from "react";

import { useBridge, useFirstSpaceOffers, useMachines, useSpaces, type Space } from "@/bridge";
import { currentLocale, useT } from "@/i18n";
import { useHiddenSpaceIds } from "@/lib/prepare/store";
import { otherComputerDesktops, pickableSpaces, type NewKind } from "@/lib/ask-first";

/**
 * What "Where it runs" offers: the kinds of new computer this Mac can make, the running Spaces you have,
 * and your other computers that are online and share their desktop.
 */
export function useWhereOptions(): { kinds: NewKind[]; spaces: Space[]; computers: Space[] } {
  const t = useT();
  const locale = currentLocale();
  const { data } = useSpaces();
  const { data: machines } = useMachines();
  const { offers } = useFirstSpaceOffers();
  const kinds = useMemo<NewKind[]>(() => {
    const k = offers.map((o) => ({ os: o.os, note: t("home.where.ready_in", { kind: o.os, time: o.time, size: o.size || "none" }), blocked: o.blocked }));
    return k.length ? k : [{ os: "linux", note: t("home.where.ready_in_few_minutes"), blocked: null }];
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `t` is new each render; the language is the dependency.
  }, [offers, locale]);
  const { core } = useBridge();
  const warming = useHiddenSpaceIds(core);
  const spaces = useMemo(() => pickableSpaces(data?.filter((s) => !warming.has(s.id))), [data, warming]);
  const here = machines?.find((m) => m.current)?.id ?? null;
  const computers = useMemo(() => otherComputerDesktops(data, here), [data, here]);
  return { kinds, spaces, computers };
}
