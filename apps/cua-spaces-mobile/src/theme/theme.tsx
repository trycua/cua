// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createContext, useContext, useMemo, type ReactNode } from "react";
import { AccessibilityInfo, Platform, useColorScheme } from "react-native";
import { useEffect, useState } from "react";

import { dark, light, type Palette } from "./tokens";

export interface Theme {
  scheme: "light" | "dark";
  c: Palette;
  /** The reader turned on Reduce Motion: no springs, no parallax, fades only. */
  reduceMotion: boolean;
  /** System monospace for commands and diffs. */
  mono: string;
}

const ThemeContext = createContext<Theme | null>(null);

/** Reads the system's Reduce Motion setting and follows changes. */
export function useReduceMotion(): boolean {
  const [on, setOn] = useState(false);
  useEffect(() => {
    let alive = true;
    AccessibilityInfo.isReduceMotionEnabled()
      .then((v) => alive && setOn(v))
      .catch(() => {});
    const sub = AccessibilityInfo.addEventListener("reduceMotionChanged", setOn);
    return () => {
      alive = false;
      sub.remove();
    };
  }, []);
  return on;
}

export function ThemeProvider({ children, force }: { children: ReactNode; force?: "light" | "dark" }) {
  const system = useColorScheme();
  const reduceMotion = useReduceMotion();
  const scheme = force ?? (system === "dark" ? "dark" : "light");
  const value = useMemo<Theme>(
    () => ({ scheme, c: scheme === "dark" ? dark : light, reduceMotion, mono: Platform.select({ ios: "Menlo", default: "monospace" }) as string }),
    [scheme, reduceMotion],
  );
  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
}

export function useTheme(): Theme {
  const t = useContext(ThemeContext);
  if (!t) throw new Error("useTheme outside ThemeProvider");
  return t;
}
