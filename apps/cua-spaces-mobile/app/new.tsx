// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useRouter } from "expo-router";

import { Gated } from "../src/features/auth/Gated";
import { StartTaskSheet } from "../src/features/task/StartTaskSheet";

/** Start a task from the phone: a sheet over Home. */
export default function NewTask() {
  const router = useRouter();
  return (
    <Gated>
      <StartTaskSheet
        onClose={() => (router.canGoBack() ? router.back() : router.replace("/"))}
        onStarted={(id) => router.replace({ pathname: "/space/[id]", params: { id } })}
      />
    </Gated>
  );
}
