// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import * as Haptics from "expo-haptics";
import { useRef, type ReactNode } from "react";
import { Pressable, View } from "react-native";
import ReanimatedSwipeable, { type SwipeableMethods } from "react-native-gesture-handler/ReanimatedSwipeable";

import type { AgentListRow } from "../../core/views";
import type { Capabilities, SpaceSummary } from "../../data/types";
import { useTheme } from "../../theme/theme";
import { Avatar } from "../../ui/Avatar";
import { Icon } from "../../ui/Icon";
import { Text } from "../../ui/Text";
import { rowTime } from "../../ui/time";
import { STATUS_WORD } from "../../data/summary";
import { StatusBadge, UnreadBadge } from "./parts";

export interface RowActions {
  onSleep?(s: SpaceSummary): void;
  onShare?(s: SpaceSummary): void;
}

/** Swipe left on a row: Sleep and Share, only what this backend can do (what it cannot is hidden, not faked). */
export function SwipeRow({ space, caps, actions, children }: { space: SpaceSummary; caps: Capabilities; actions: RowActions; children: ReactNode }) {
  const { c } = useTheme();
  const ref = useRef<SwipeableMethods>(null);
  const canSleep = caps.sleep && space.canSleep && space.tone !== "asleep" && space.machineOnline && !!actions.onSleep;
  const canShare = caps.share && space.canShare && !!actions.onShare;
  if (!canSleep && !canShare) return <>{children}</>;
  const act = (label: string, bg: string, icon: "moon" | "share", run: () => void) => (
    <Pressable
      key={label}
      accessibilityRole="button"
      accessibilityLabel={`${label} ${space.name}`}
      onPress={() => {
        Haptics.selectionAsync().catch(() => {});
        ref.current?.close();
        run();
      }}
      style={{ width: 76, backgroundColor: bg, alignItems: "center", justifyContent: "center", gap: 3 }}
    >
      <Icon name={icon} size={20} color="#fff" />
      <Text variant="caption" weight="600" style={{ color: "#fff" }}>
        {label}
      </Text>
    </Pressable>
  );
  return (
    <ReanimatedSwipeable
      ref={ref}
      friction={2}
      overshootRight={false}
      rightThreshold={36}
      renderRightActions={() => (
        <View style={{ flexDirection: "row" }}>
          {canSleep ? act("Sleep", "#6a5acd", "moon", () => actions.onSleep!(space)) : null}
          {canShare ? act("Share", c.brandStrong, "share", () => actions.onShare!(space)) : null}
        </View>
      )}
    >
      {children}
    </ReanimatedSwipeable>
  );
}

const preview = (s: SpaceSummary) => (s.lastMessage ? `${s.lastMessage.author}: ${s.lastMessage.text}` : s.statusLine || "No messages yet");

/**
 * An iMessage style row for an agent (avatar, name, last message, time, unread badge, one status), or for a Space
 * with no agent yet (a machine tile in place of the avatar), so every Space can be opened from the list.
 */
export function ConversationRow({
  row,
  space,
  now,
  onPress,
  onLongPress,
}: {
  row: AgentListRow | null;
  space: SpaceSummary;
  now: number;
  onPress(): void;
  onLongPress?(): void;
}) {
  const { c } = useTheme();
  const last = row?.lastMs || space.lastMessage?.tsMs || 0;
  const unread = row ? row.unread : space.unread;
  const tone = space.machineOnline ? (row ? row.tone : space.tone) : "stopped";
  const word = space.machineOnline ? STATUS_WORD[tone] : "Offline";
  const title = row ? row.label : space.name;
  return (
    <Pressable
      onPress={onPress}
      onLongPress={onLongPress}
      delayLongPress={350}
      accessibilityRole="button"
      accessibilityLabel={`${title}${row ? `, in ${space.name}` : ""}, on ${space.machineName}, ${word}${unread ? `, ${unread} unread` : ""}. ${preview(space)}`}
      accessibilityHint="Opens the chat. Swipe left for more actions."
      style={({ pressed }) => ({
        flexDirection: "row",
        gap: 12,
        paddingHorizontal: 16,
        paddingVertical: 10,
        backgroundColor: pressed ? c.accent : c.background,
      })}
    >
      <View style={{ opacity: space.machineOnline ? 1 : 0.55 }}>
        {row ? (
          <Avatar name={row.label} kind="agent" size={52} dot={space.machineOnline && row.tone === "working" ? c.success : null} />
        ) : (
          <View style={{ width: 52, height: 52, borderRadius: 26, backgroundColor: c.tile, alignItems: "center", justifyContent: "center" }}>
            <Icon name={space.scope === "app" ? "pc" : "mac"} size={22} color={c.mutedForeground} />
          </View>
        )}
      </View>
      <View style={{ flex: 1, gap: 2 }}>
        <View style={{ flexDirection: "row", alignItems: "baseline", gap: 8 }}>
          <Text variant="headline" numberOfLines={1} style={{ flexShrink: 1, opacity: space.machineOnline ? 1 : 0.6 }}>
            {title}
          </Text>
          <View style={{ flex: 1 }} />
          <Text variant="footnote" color="muted">
            {last ? rowTime(last, now) : ""}
          </Text>
        </View>
        <View style={{ flexDirection: "row", alignItems: "center", gap: 8 }}>
          <Text variant="subhead" color="muted" numberOfLines={2} style={{ flex: 1 }}>
            {preview(space)}
          </Text>
          <UnreadBadge n={unread} />
        </View>
        <View style={{ flexDirection: "row", alignItems: "center", gap: 10, marginTop: 2 }}>
          <StatusBadge tone={tone} word={word} />
          <Text variant="footnote" color="muted" numberOfLines={1} style={{ flexShrink: 1 }}>
            {space.name === space.machineName || !row ? space.machineName : `${space.name} on ${space.machineName}`}
          </Text>
        </View>
      </View>
    </Pressable>
  );
}
