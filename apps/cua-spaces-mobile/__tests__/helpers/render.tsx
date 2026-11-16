// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { render } from "@testing-library/react-native";
import type { ReactElement } from "react";

import type { Backend } from "../../src/data/types";
import { createDemoBackend } from "../../src/data/demo/backend";
import { BackendProvider, clearThreadMemory } from "../../src/state/backend";
import { StoresProvider } from "../../src/state/outbox";
import { ThemeProvider } from "../../src/theme/theme";
import type { SealedStore } from "../../src/store/sealed";
import { testStore } from "./store";

export const NOW = 1_800_000_000_000;

const me = () => "acct:you";
/** Ids the outbox gives its messages: m1, m2, ... per render, so every core call the tests make is the same each run. */
const fixedNow = () => NOW;

export async function renderWithApp(
  ui: ReactElement,
  opts: { scheme?: "light" | "dark"; wrap?: (b: Backend) => Backend; store?: SealedStore; offline?: boolean; createMs?: number } = {},
) {
  clearThreadMemory();
  const demo = createDemoBackend({ now: () => NOW, wordMs: 0, offline: opts.offline, createMs: opts.createMs });
  const backend = opts.wrap ? opts.wrap(demo) : demo;
  const store = opts.store ?? testStore();
  let n = 0;
  const newId = () => `m${++n}`;
  const utils = await render(
    <ThemeProvider force={opts.scheme ?? "light"}>
      <BackendProvider backend={backend}>
        <StoresProvider store={store} me={me} now={fixedNow} newId={newId}>
          {ui}
        </StoresProvider>
      </BackendProvider>
    </ThemeProvider>,
  );
  return { ...utils, backend, demo, store };
}
