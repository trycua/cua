// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useLocalSearchParams, useRouter } from "expo-router";
import { useState } from "react";

import { ensureUnlocked, needsUnlock, useLock, VIEWER_FRESH_MS } from "../../src/auth/lock";
import { Gated } from "../../src/features/auth/Gated";
import { LockScreen } from "../../src/features/auth/LockScreen";
import { StreamView } from "../../src/features/stream/StreamView";
import { useBackend, useSpaceList } from "../../src/state/backend";
import { usePrefs } from "../../src/state/prefs";

function Stream({ id }: { id: string }) {
  const router = useRouter();
  const backend = useBackend();
  const { list } = useSpaceList();
  const [controlled, setControlled] = useState(false);
  const scene = list?.spaces.find((s) => s.id === id)?.scene ?? "blank";
  return (
    <StreamView
      spaceId={id}
      scene={scene}
      mode="full"
      controlled={controlled}
      onClose={() => router.back()}
      onToggleControl={async (on) => {
        // Taking control of a desktop needs an unlock no older than five minutes.
        if (on && !(await ensureUnlocked("Take control of this computer", usePrefs.getState().requireUnlock && !backend.capabilities.sample))) return;
        setControlled(on);
        await backend.takeControl(id, on).catch(() => setControlled(!on));
      }}
    />
  );
}

/** The desktop of a computer needs an unlock no older than five minutes. */
function Fresh({ children }: { children: React.ReactNode }) {
  const backend = useBackend();
  const required = usePrefs((p) => p.requireUnlock) && !backend.capabilities.sample;
  const unlockedAt = useLock((s) => s.unlockedAt);
  if (needsUnlock({ required, unlockedAt, now: Date.now(), maxAgeMs: VIEWER_FRESH_MS }))
    return <LockScreen title="Unlock to see this desktop" reason="Open the desktop of a computer" />;
  return <>{children}</>;
}

export default function StreamRoute() {
  const { id } = useLocalSearchParams<{ id: string }>();
  return (
    <Gated>
      <Fresh>
        <Stream id={id} />
      </Fresh>
    </Gated>
  );
}
