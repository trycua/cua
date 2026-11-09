// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Text as RNText, type TextProps, type TextStyle } from "react-native";

import { useTheme } from "../theme/theme";

export type Variant = "largeTitle" | "title" | "headline" | "body" | "callout" | "subhead" | "footnote" | "caption" | "mono";

/** iOS text styles (Dynamic Type scales all of them); one place so the sizes stay native. */
export const TYPE: Record<Variant, TextStyle> = {
  largeTitle: { fontSize: 32, lineHeight: 38, fontWeight: "700", letterSpacing: 0.2 },
  title: { fontSize: 22, lineHeight: 28, fontWeight: "600" },
  headline: { fontSize: 17, lineHeight: 22, fontWeight: "600" },
  body: { fontSize: 17, lineHeight: 22, fontWeight: "400" },
  callout: { fontSize: 16, lineHeight: 21, fontWeight: "400" },
  subhead: { fontSize: 15, lineHeight: 20, fontWeight: "400" },
  footnote: { fontSize: 13, lineHeight: 18, fontWeight: "400" },
  caption: { fontSize: 12, lineHeight: 16, fontWeight: "400" },
  mono: { fontSize: 13, lineHeight: 18, fontWeight: "400" },
};

export interface Props extends TextProps {
  variant?: Variant;
  color?: "foreground" | "muted" | "brand" | "warning" | "success" | "destructive" | "inverse";
  weight?: TextStyle["fontWeight"];
}

export function Text({ variant = "body", color = "foreground", weight, style, ...rest }: Props) {
  const { c, mono } = useTheme();
  const col = {
    foreground: c.foreground,
    muted: c.mutedForeground,
    brand: c.brandStrong,
    warning: c.warning,
    success: c.success,
    destructive: c.destructive,
    inverse: c.primaryForeground,
  }[color];
  return (
    <RNText
      // Dynamic Type: text follows the reader's size up to the accessibility sizes.
      maxFontSizeMultiplier={variant === "caption" || variant === "footnote" ? 2 : 2.4}
      {...rest}
      style={[TYPE[variant], variant === "mono" ? { fontFamily: mono } : null, { color: col }, weight ? { fontWeight: weight } : null, style]}
    />
  );
}
