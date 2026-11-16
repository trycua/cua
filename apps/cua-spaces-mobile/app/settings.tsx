// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useRouter } from "expo-router";

import { Gated } from "../src/features/auth/Gated";
import { SettingsScreen } from "../src/features/settings/SettingsScreen";

export default function Settings() {
  const router = useRouter();
  return (
    <Gated>
      <SettingsScreen onClose={() => router.back()} />
    </Gated>
  );
}
