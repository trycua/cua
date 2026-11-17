// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect } from "react";

import { useSettings } from "@/bridge";
import { create } from "zustand";

export type ThemePreference = "system" | "light" | "dark";

// Keep in sync with the inline script in index.html.
export const THEME_STORAGE_KEY = "cua-spaces:theme";
const BACKGROUND = { light: "#f7f8fa", dark: "#16181c" } as const;

function readStored(): ThemePreference {
  try {
    const v = localStorage.getItem(THEME_STORAGE_KEY);
    if (v === "light" || v === "dark" || v === "system") return v;
  } catch {
    // Storage can be unavailable in sandboxed hosts.
  }
  return "system";
}

function systemPrefersDark(): boolean {
  return typeof window !== "undefined" && window.matchMedia("(prefers-color-scheme: dark)").matches;
}

export function resolveTheme(pref: ThemePreference, prefersDark: boolean): "light" | "dark" {
  if (pref === "system") return prefersDark ? "dark" : "light";
  return pref;
}

export function applyTheme(resolved: "light" | "dark", root: HTMLElement = document.documentElement): void {
  root.classList.toggle("dark", resolved === "dark");
  root.style.colorScheme = resolved;
  root.style.backgroundColor = BACKGROUND[resolved];
}

interface ThemeState {
  preference: ThemePreference;
  setPreference: (p: ThemePreference) => void;
}

export const useThemeStore = create<ThemeState>((set) => ({
  preference: typeof window === "undefined" ? "system" : readStored(),
  setPreference: (preference) => {
    try {
      localStorage.setItem(THEME_STORAGE_KEY, preference);
    } catch {
      // Ignore; the choice still applies for this session.
    }
    set({ preference });
  },
}));

/** Keeps <html> in step with the preference and, for "system", the OS. */
export function useThemeSync(): void {
  const preference = useThemeStore((s) => s.preference);
  useEffect(() => {
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const update = () => applyTheme(resolveTheme(preference, media.matches));
    update();
    if (preference !== "system") return;
    media.addEventListener("change", update);
    return () => media.removeEventListener("change", update);
  }, [preference]);
}

export function useTheme() {
  const preference = useThemeStore((s) => s.preference);
  const setPreference = useThemeStore((s) => s.setPreference);
  return { preference, setPreference, resolved: resolveTheme(preference, systemPrefersDark()) };
}

/** Tells the host (the Electron window, the native app) when the appearance changes here. */
export function useThemeHostSync(): void {
  const preference = useThemeStore((s) => s.preference);
  const { data, updateSetting } = useSettings();
  const hostTheme = data?.values.theme;
  useEffect(() => {
    if (hostTheme !== undefined && hostTheme !== preference) void updateSetting("theme", preference).catch(() => {});
    // updateSetting is a new function each render; the values decide.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [hostTheme, preference]);
}
