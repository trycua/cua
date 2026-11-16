// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Image } from "expo-image";
import { View } from "react-native";

import { useTheme } from "../theme/theme";
import { Icon } from "./Icon";
import { Text } from "./Text";

const HUES = ["#d9822b", "#8a56c9", "#3b7fd9", "#c24a8a", "#2f9e7f", "#c9553b", "#5a6ad9"];
const hash = (s: string) => [...s].reduce((a, ch) => (a * 31 + ch.charCodeAt(0)) >>> 0, 7);

/** The Cua Spaces app icon: the avatar of Cua's own entries. */
// eslint-disable-next-line @typescript-eslint/no-require-imports
export const CUA_APP_ICON = require("../../assets/cua-app-icon-256.png");

export interface AvatarProps {
  name: string;
  kind: "person" | "agent" | "system";
  size?: number;
  /** A small presence dot at the corner. */
  dot?: string | null;
}

export function Avatar({ name, kind, size = 32, dot }: AvatarProps) {
  const { c } = useTheme();
  let body;
  if (kind === "system") {
    body = <Image source={CUA_APP_ICON} style={{ width: size, height: size, borderRadius: size * 0.225 }} accessibilityLabel="Cua app" />;
  } else {
    const bg = kind === "agent" ? "#1f8a78" : HUES[hash(name) % HUES.length]!;
    body = (
      <View style={{ width: size, height: size, borderRadius: size / 2, backgroundColor: bg, alignItems: "center", justifyContent: "center" }}>
        {kind === "agent" ? (
          <Icon name="bolt" size={size * 0.5} color="#fff" />
        ) : (
          <Text variant="caption" weight="600" style={{ color: "#fff", fontSize: size * 0.42, lineHeight: size * 0.5 }} allowFontScaling={false}>
            {name.slice(0, 1).toUpperCase()}
          </Text>
        )}
      </View>
    );
  }
  return (
    <View style={{ width: size, height: size }} accessible={false}>
      {body}
      {dot ? (
        <View
          style={{
            position: "absolute",
            right: -1,
            bottom: -1,
            width: size * 0.3,
            height: size * 0.3,
            borderRadius: size,
            backgroundColor: dot,
            borderWidth: 2,
            borderColor: c.background,
          }}
        />
      ) : null}
    </View>
  );
}

/** A small overlapping stack of members (header, rows). */
export function AvatarStack({
  members,
  size = 26,
  max = 3,
}: {
  members: { name: string; kind: "person" | "agent" | "system" }[];
  size?: number;
  max?: number;
}) {
  const { c } = useTheme();
  const shown = members.slice(0, max);
  return (
    <View style={{ flexDirection: "row" }} accessible accessibilityLabel={`${members.length} members`}>
      {shown.map((m, i) => (
        <View key={m.name + i} style={{ marginLeft: i === 0 ? 0 : -size * 0.3, borderRadius: size, borderWidth: 2, borderColor: c.background }}>
          <Avatar name={m.name} kind={m.kind} size={size} />
        </View>
      ))}
    </View>
  );
}
