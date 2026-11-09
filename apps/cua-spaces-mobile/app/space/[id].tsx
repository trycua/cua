// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Redirect, useLocalSearchParams, useRouter } from "expo-router";

import { Gated } from "../../src/features/auth/Gated";
import { useLayout } from "../../src/layout/layout";
import { ThreadScreen } from "../../src/features/thread/ThreadScreen";

/** A Space as a chat: a screen on phones; wide windows show it beside the list. */
export default function SpaceRoute() {
  const { id, entry, stream } = useLocalSearchParams<{ id: string; entry?: string; stream?: string }>();
  const router = useRouter();
  const layout = useLayout();
  if (layout !== "phone") return <Redirect href={{ pathname: "/", params: { space: id, ...(entry ? { entry } : {}) } }} />;
  return (
    <Gated>
      <ThreadScreen
        spaceId={id}
        focusSeq={entry ? Number(entry) : null}
        initialStreamOpen={stream === "1"}
        onBack={() => (router.canGoBack() ? router.back() : router.replace("/"))}
        onFullStream={() => router.push({ pathname: "/stream/[id]", params: { id } })}
        onOpenChanges={() => router.push({ pathname: "/diff/[id]", params: { id } })}
      />
    </Gated>
  );
}
