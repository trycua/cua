// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
// The webview build has no Node types; tests run in Node.
// @ts-expect-error node:fs is untyped here
import { readFileSync } from "node:fs";
import { afterEach } from "vitest";

import { initCoreSync } from "../core";

// The app core (wasm), loaded once before any test imports a model.
declare const process: { cwd(): string };
initCoreSync(readFileSync(`${process.cwd()}/src/core/wasm/core_bg.wasm`));

afterEach(() => {
  cleanup();
});

// Recent Node exposes `localStorage` only behind --localstorage-file, and jsdom
// defers to the host's when one is present, so `window.localStorage` can be
// undefined here even though it always exists in a real WebView. Tests that
// clear it between cases then fail at setup rather than on an assertion.
if (typeof window !== "undefined" && !window.localStorage) {
  const store = new Map<string, string>();
  Object.defineProperty(window, "localStorage", {
    configurable: true,
    value: {
      get length() {
        return store.size;
      },
      key: (i: number) => [...store.keys()][i] ?? null,
      getItem: (k: string) => store.get(String(k)) ?? null,
      setItem: (k: string, v: string) => void store.set(String(k), String(v)),
      removeItem: (k: string) => void store.delete(String(k)),
      clear: () => store.clear(),
    } as Storage,
  });
}

// jsdom does not implement matchMedia; the app uses it for reduced-motion.
if (typeof window !== "undefined" && !window.matchMedia) {
  window.matchMedia = (query: string) =>
    ({
      matches: false,
      media: query,
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    }) as MediaQueryList;
}
