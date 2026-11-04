// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useLocalSearchParams, useRouter } from "expo-router";

import { Gated } from "../../src/features/auth/Gated";
import { DiffScreen } from "../../src/features/diff/DiffScreen";

/** What an agent changed in a Space: from the thread's header and from the push approval card. */
export default function DiffRoute() {
  const { id, path } = useLocalSearchParams<{ id: string; path?: string }>();
  const router = useRouter();
  return (
    <Gated>
      <DiffScreen spaceId={id} initialPath={path ?? null} onBack={() => (router.canGoBack() ? router.back() : router.replace("/"))} />
    </Gated>
  );
}
