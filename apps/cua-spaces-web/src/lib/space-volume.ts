// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useState } from "react";

import { spaceVolumeErrors, useBridge } from "@/bridge";

/** A Space's details: the daemon's reason it has no Cua Volume, when it
 * connected without one (null otherwise, or while unknown). Read when the
 * Space runs; a host without the daemon's volume tools answers null. */
export function useSpaceVolumeError(space: { id: string; name: string } | undefined, running: boolean): string | null {
  const { data } = useBridge();
  const [note, setNote] = useState<string | null>(null);
  const id = space?.id;
  const name = space?.name;
  useEffect(() => {
    setNote(null);
    if (!data || !id || !name || !running) return;
    let live = true;
    data.call("volume.overview", {}).then(
      (o) => live && setNote(spaceVolumeErrors(o.sync, o.mount).find((e) => e.space === id || e.space === name)?.error ?? null),
      () => {},
    );
    return () => {
      live = false;
    };
  }, [data, id, name, running]);
  return note;
}
