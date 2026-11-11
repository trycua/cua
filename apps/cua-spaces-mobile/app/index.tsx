// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useLocalSearchParams, useRouter } from "expo-router";

import { Gated } from "../src/features/auth/Gated";
import { Workspace } from "../src/features/Workspace";

export default function Index() {
  const { space, entry } = useLocalSearchParams<{ space?: string; entry?: string }>();
  const router = useRouter();
  return (
    <Gated>
      <Workspace
        selected={space ?? null}
        entry={entry ? Number(entry) : null}
        onSelect={(id) => router.setParams({ space: id ?? undefined, entry: undefined })}
      />
    </Gated>
  );
}
