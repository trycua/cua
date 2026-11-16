// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Pressable, ScrollView, View } from "react-native";

import type { AgentTone, ComputerRow } from "../../core/views";
import type { MachineInfo, SpaceSummary } from "../../data/types";
import { useTheme } from "../../theme/theme";
import { Icon, type IconName } from "../../ui/Icon";
import { Text } from "../../ui/Text";
import { Thumbnail } from "../../ui/Thumbnail";

export function toneColor(tone: AgentTone, c: ReturnType<typeof useTheme>["c"]): string {
  switch (tone) {
    case "you":
    case "not-connected":
      return c.warning;
    case "working":
    case "creating":
      return c.success;
    case "failed":
      return c.destructive;
    default:
      return c.mutedForeground;
  }
}

/** One status per row: Needs you (filled), Running (green), Asleep (moon), or the plain word. */
export function StatusBadge({ tone, word }: { tone: AgentTone; word: string }) {
  const { c } = useTheme();
  const col = toneColor(tone, c);
  if (tone === "you") {
    return (
      <View style={{ backgroundColor: c.warning, borderRadius: 999, paddingHorizontal: 8, paddingVertical: 2 }} accessibilityLabel={word}>
        <Text variant="caption" weight="600" style={{ color: "#1a1200" }}>
          {word}
        </Text>
      </View>
    );
  }
  return (
    <View style={{ flexDirection: "row", alignItems: "center", gap: 5 }} accessibilityLabel={word}>
      {tone === "asleep" ? <Icon name="moon" size={12} color={col} /> : <View style={{ width: 7, height: 7, borderRadius: 4, backgroundColor: col }} />}
      <Text variant="footnote" style={{ color: col }}>
        {word}
      </Text>
    </View>
  );
}

export function UnreadBadge({ n }: { n: number }) {
  const { c } = useTheme();
  if (n <= 0) return null;
  return (
    <View
      style={{
        minWidth: 20,
        height: 20,
        paddingHorizontal: 6,
        borderRadius: 10,
        backgroundColor: c.brandStrong,
        alignItems: "center",
        justifyContent: "center",
      }}
      accessibilityLabel={`${n} unread`}
    >
      <Text variant="caption" weight="600" style={{ color: "#fff" }} allowFontScaling={false}>
        {n}
      </Text>
    </View>
  );
}

/** Filter by computer: one list, the computer is state on the row. */
export function MachineChips({ machines, value, onChange }: { machines: MachineInfo[]; value: string | null; onChange(id: string | null): void }) {
  const { c } = useTheme();
  const chip = (id: string | null, label: string, online: boolean) => {
    const on = value === id;
    return (
      <Pressable
        key={id ?? "all"}
        onPress={() => onChange(id)}
        accessibilityRole="button"
        accessibilityState={{ selected: on }}
        accessibilityLabel={id ? `${label}, ${online ? "online" : "offline"}` : "All computers"}
        style={{
          flexDirection: "row",
          alignItems: "center",
          gap: 6,
          paddingHorizontal: 12,
          paddingVertical: 5,
          minHeight: 32,
          borderRadius: 18,
          backgroundColor: on ? c.primary : c.tile,
        }}
      >
        {id ? <View style={{ width: 6, height: 6, borderRadius: 3, backgroundColor: online ? c.success : c.mutedForeground }} /> : null}
        <Text variant="subhead" weight="500" style={{ color: on ? c.primaryForeground : c.foreground }}>
          {label}
        </Text>
      </Pressable>
    );
  };
  return (
    <ScrollView horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={{ paddingHorizontal: 16, gap: 8 }} keyboardShouldPersistTaps="handled">
      {chip(null, "All", true)}
      {machines.map((m) => chip(m.id, m.name, m.online))}
    </ScrollView>
  );
}

/** Stories-style bar: live 16:9 thumbnails, a small live dot, the name under each, a ring for Needs you. */
export function StoriesBar({ spaces, onOpen }: { spaces: SpaceSummary[]; onOpen(id: string): void }) {
  const { c } = useTheme();
  return (
    <ScrollView
      horizontal
      showsHorizontalScrollIndicator={false}
      contentContainerStyle={{ paddingHorizontal: 16, gap: 12, paddingVertical: 4 }}
      accessibilityLabel="Spaces"
    >
      {spaces.map((s) => {
        const needs = s.tone === "you";
        const live = s.machineOnline && s.tone !== "asleep" && s.tone !== "stopped";
        return (
          <Pressable
            key={s.id}
            onPress={() => onOpen(s.id)}
            accessibilityRole="button"
            accessibilityLabel={`${s.name}, ${s.statusWord}`}
            style={{ width: 128 }}
          >
            <View
              style={{
                padding: 2.5,
                borderRadius: 12,
                borderWidth: needs ? 2.5 : 0,
                borderColor: c.warning,
                margin: needs ? 0 : 2.5,
                opacity: s.machineOnline ? 1 : 0.5,
              }}
            >
              <Thumbnail scene={s.scene} live={live} radius={8} osIcon={s.scope === "app" ? "pc" : "mac"} seed={s.name.length} />
            </View>
            <Text variant="caption" numberOfLines={1} style={{ marginTop: 4, textAlign: "center" }} weight={needs ? "600" : "400"}>
              {s.name}
            </Text>
          </Pressable>
        );
      })}
    </ScrollView>
  );
}

/** The glyph of a computer or phone of the pool (a phone is never drawn as a Mac). */
const COMPUTER_ICON: Record<ComputerRow["kind"], IconName> = {
  mac: "mac",
  windows: "pc",
  linux: "linux",
  cloud: "cloud",
  iphone: "iphone",
  ipad: "ipad",
  phone: "iphone",
};

/** The pool's computers and phones, each with its state: a computer is never a row of the Spaces list. */
export function ComputersSection({ computers }: { computers: ComputerRow[] }) {
  const { c } = useTheme();
  if (!computers.length) return null;
  return (
    <View testID="computers" style={{ marginTop: 18, paddingHorizontal: 16 }} accessibilityLabel="Computers">
      <Text variant="footnote" weight="600" color="muted" accessibilityRole="header" style={{ textTransform: "uppercase", marginBottom: 6 }}>
        Computers
      </Text>
      {computers.map((m) => (
        <View
          key={m.id}
          testID={`computer-${m.id}`}
          accessibilityLabel={[m.name, m.current ? "this phone" : "", m.state, m.line, m.note].filter(Boolean).join(", ")}
          style={{ flexDirection: "row", alignItems: "center", gap: 12, paddingVertical: 10, opacity: m.online ? 1 : 0.6 }}
        >
          <View style={{ width: 34, height: 34, borderRadius: 9, backgroundColor: c.tile, alignItems: "center", justifyContent: "center" }}>
            <Icon name={COMPUTER_ICON[m.kind]} size={18} color={c.foreground} />
          </View>
          <View style={{ flex: 1 }}>
            <View style={{ flexDirection: "row", alignItems: "center", gap: 6 }}>
              <Text variant="body" weight="500" numberOfLines={1} style={{ flexShrink: 1 }}>
                {m.name}
              </Text>
              {m.current ? (
                <Text variant="caption" color="muted">
                  This phone
                </Text>
              ) : null}
            </View>
            <Text variant="footnote" color="muted" numberOfLines={1}>
              {[m.state, m.line].filter(Boolean).join(", ")}
            </Text>
            {m.note ? (
              <Text variant="footnote" color="warning">
                {m.note}
              </Text>
            ) : null}
            {m.unconfirmed ? (
              <Text variant="footnote" color="warning">
                New. Confirm it in Cua Spaces on one of your computers before using it.
              </Text>
            ) : null}
          </View>
          <View style={{ width: 8, height: 8, borderRadius: 4, backgroundColor: m.online ? c.success : c.mutedForeground }} />
        </View>
      ))}
    </View>
  );
}
