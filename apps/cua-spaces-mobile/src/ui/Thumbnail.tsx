// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { memo } from "react";
import { View, type ViewStyle } from "react-native";

import type { Scene } from "../data/types";
import { useTheme } from "../theme/theme";
import { Icon } from "./Icon";

/** Bars of a made-up document, deterministic by seed. */
const bars = (n: number, seed: number) => Array.from({ length: n }, (_, i) => 0.25 + ((i * 37 + seed * 11) % 60) / 100);

function Bars({ n, seed, color, h = 3, gap = 3 }: { n: number; seed: number; color: string | string[]; h?: number; gap?: number }) {
  return (
    // The bars spread over the height they are given, so a big stream shows a full document, not a strip.
    <View style={{ gap, flex: 1, justifyContent: "space-between" }}>
      {bars(n, seed).map((w, i) => (
        <View key={i} style={{ height: h, width: `${w * 100}%`, borderRadius: 2, backgroundColor: Array.isArray(color) ? color[i % color.length] : color }} />
      ))}
    </View>
  );
}

function Window({ children, bg, bar, style }: { children?: React.ReactNode; bg: string; bar: string; style?: ViewStyle }) {
  return (
    <View style={[{ backgroundColor: bg, borderRadius: 3, overflow: "hidden" }, style]}>
      <View style={{ height: 6, backgroundColor: bar, flexDirection: "row", alignItems: "center", paddingLeft: 3, gap: 2 }}>
        {["#ff5f57", "#febc2e", "#28c840"].map((d) => (
          <View key={d} style={{ width: 2.5, height: 2.5, borderRadius: 2, backgroundColor: d }} />
        ))}
      </View>
      <View style={{ flex: 1, padding: 4 }}>{children}</View>
    </View>
  );
}

/**
 * A 16:9 picture of a Space's desktop. A live Space shows its stream; this
 * draws the scene of a sample Space, or a quiet machine tile when no picture
 * exists yet. The live dot says the stream is up.
 */
export const Thumbnail = memo(function Thumbnail({
  scene,
  live = false,
  radius = 8,
  osIcon = "mac" as const,
  seed = 1,
  style,
}: {
  scene: Scene;
  live?: boolean;
  radius?: number;
  osIcon?: "mac" | "pc" | "cloud" | "linux";
  seed?: number;
  style?: ViewStyle;
}) {
  const { c, scheme } = useTheme();
  const dark = scheme === "dark";
  let inner;
  switch (scene) {
    case "code":
      inner = (
        <View style={{ flex: 1, backgroundColor: "#15171c", padding: 5, flexDirection: "row", gap: 4 }}>
          <View style={{ width: "18%", gap: 3 }}>
            <Bars n={6} seed={seed} color="#2c303a" />
          </View>
          <View style={{ flex: 1 }}>
            <Bars n={11} seed={seed + 3} color={["#6aa8ff", "#e0a458", "#8bd5a0", "#6c7280"]} />
          </View>
        </View>
      );
      break;
    case "browser":
      inner = (
        <View style={{ flex: 1, backgroundColor: "#cfe3f1", padding: 5 }}>
          <Window bg="#fbfbfc" bar="#e9ebee" style={{ flex: 1 }}>
            <View style={{ height: 5, borderRadius: 3, backgroundColor: "#e9ebee", marginBottom: 4 }} />
            <View style={{ flexDirection: "row", gap: 4, flex: 1 }}>
              <View style={{ flex: 1, backgroundColor: "#cfd8e3", borderRadius: 2 }} />
              <View style={{ flex: 1.2 }}>
                <Bars n={6} seed={seed} color="#c3c8d0" />
              </View>
            </View>
          </Window>
        </View>
      );
      break;
    case "notes":
      inner = (
        <View style={{ flex: 1, backgroundColor: "#f4e9b8", padding: 8 }}>
          <Bars n={9} seed={seed} color="#c9b979" h={2.5} gap={4} />
        </View>
      );
      break;
    case "terminal":
      inner = (
        <View style={{ flex: 1, backgroundColor: "#0e1013", padding: 6 }}>
          <Bars n={10} seed={seed} color={["#5ad17b", "#8a919c", "#8a919c", "#e0a458"]} h={2.5} gap={4} />
        </View>
      );
      break;
    case "design":
      inner = (
        <View style={{ flex: 1, backgroundColor: dark ? "#2a2d33" : "#e8e9ed", padding: 6, flexDirection: "row", gap: 5 }}>
          <View style={{ flex: 1, backgroundColor: "#ffffff", borderRadius: 2, borderWidth: 1.5, borderColor: "#3b82f6" }} />
          <View style={{ flex: 1, backgroundColor: "#ffffff", borderRadius: 2 }} />
          <View style={{ width: "16%", gap: 3 }}>
            <Bars n={5} seed={seed} color={dark ? "#444850" : "#cfd2d9"} />
          </View>
        </View>
      );
      break;
    case "desktop":
      inner = (
        <View style={{ flex: 1, backgroundColor: "#35607f", padding: 6 }}>
          <Window bg="#f5f6f8" bar="#dfe2e7" style={{ width: "62%", height: "70%" }}>
            <Bars n={4} seed={seed} color="#c3c8d0" />
          </Window>
          <View
            style={{
              position: "absolute",
              bottom: 3,
              alignSelf: "center",
              flexDirection: "row",
              gap: 3,
              backgroundColor: "#ffffff30",
              padding: 2,
              borderRadius: 4,
            }}
          >
            {["#4f8cff", "#ff8a3d", "#43c06e", "#b36bff"].map((d) => (
              <View key={d} style={{ width: 6, height: 6, borderRadius: 2, backgroundColor: d }} />
            ))}
          </View>
        </View>
      );
      break;
    default:
      inner = (
        <View style={{ flex: 1, backgroundColor: c.thumb, alignItems: "center", justifyContent: "center" }}>
          <Icon name={osIcon} size={22} color={c.mutedForeground} />
        </View>
      );
  }
  return (
    <View
      style={[{ aspectRatio: 16 / 9, borderRadius: radius, overflow: "hidden", backgroundColor: c.thumb, borderWidth: 0.5, borderColor: c.border }, style]}
      accessibilityRole="image"
      accessibilityLabel={live ? "Live desktop" : "Desktop preview"}
    >
      {inner}
      {live ? (
        <View
          style={{
            position: "absolute",
            top: 4,
            right: 4,
            width: 7,
            height: 7,
            borderRadius: 4,
            backgroundColor: "#ff3b30",
            borderWidth: 1,
            borderColor: "#fff",
          }}
        />
      ) : null}
    </View>
  );
});
