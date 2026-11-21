// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useRouter } from "expo-router";
import { useCallback, useState } from "react";
import { View } from "react-native";

import { useBackend } from "../state/backend";
import { useTheme } from "../theme/theme";
import { Text } from "../ui/Text";
import { useLayout } from "../layout/layout";
import { HomeScreen } from "./home/HomeScreen";
import { StreamView } from "./stream/StreamView";
import { ThreadScreen } from "./thread/ThreadScreen";
import { useSpaceList } from "../state/backend";

/** Home, and on wide windows the thread (and the live desktop) beside it. */
export function Workspace({ selected, onSelect, entry }: { selected: string | null; onSelect(id: string | null): void; entry?: number | null }) {
  const { c } = useTheme();
  const layout = useLayout();
  const router = useRouter();
  const backend = useBackend();
  const { list } = useSpaceList();
  const [controlled, setControlled] = useState(false);
  const space = list?.spaces.find((s) => s.id === selected);

  const open = useCallback(
    (id: string) => {
      if (layout === "phone") router.push({ pathname: "/space/[id]", params: { id } });
      else onSelect(id);
    },
    [layout, router, onSelect],
  );
  const settings = useCallback(() => router.push("/settings"), [router]);
  const newTask = useCallback(() => router.push("/new"), [router]);
  const toggleControl = useCallback(
    async (on: boolean) => {
      setControlled(on);
      if (selected) await backend.takeControl(selected, on).catch(() => setControlled(!on));
    },
    [backend, selected],
  );
  const home = <HomeScreen onOpen={open} onSettings={settings} onNewTask={newTask} />;
  if (layout === "phone") return home;

  const empty = (
    <View style={{ flex: 1, alignItems: "center", justifyContent: "center", gap: 6, backgroundColor: c.background }}>
      <Text variant="headline">Pick a Space</Text>
      <Text color="muted">Its chat opens here.</Text>
    </View>
  );
  const thread = selected ? (
    <ThreadScreen
      key={selected}
      spaceId={selected}
      focusSeq={entry ?? null}
      onFullStream={() => router.push({ pathname: "/stream/[id]", params: { id: selected } })}
      onOpenChanges={() => router.push({ pathname: "/diff/[id]", params: { id: selected } })}
      hideHeaderStream={layout === "triple" || layout === "duo"}
      onBack={layout === "duo" ? () => onSelect(null) : undefined}
    />
  ) : (
    empty
  );
  const stream = selected ? (
    <StreamView
      spaceId={selected}
      scene={space?.scene ?? "blank"}
      mode="full"
      embedded
      controlled={controlled}
      onClose={() => {}}
      onToggleControl={toggleControl}
    />
  ) : (
    <View style={{ flex: 1, backgroundColor: "#000" }} />
  );
  const rule = { width: 0.5, backgroundColor: c.border };

  if (layout === "split") {
    return (
      <View style={{ flex: 1, flexDirection: "row" }}>
        <View style={{ width: 340 }}>{home}</View>
        <View style={rule} />
        <View style={{ flex: 1 }}>{thread}</View>
      </View>
    );
  }
  if (layout === "duo") {
    // The list is the screen behind; open, the chat takes the left half and the desktop the right.
    if (!selected) return home;
    return (
      <View style={{ flex: 1, flexDirection: "row" }}>
        <View style={{ flex: 1 }}>{thread}</View>
        <View style={rule} />
        <View style={{ flex: 1 }}>{stream}</View>
      </View>
    );
  }
  return (
    <View style={{ flex: 1, flexDirection: "row" }}>
      <View style={{ width: 360 }}>{home}</View>
      <View style={rule} />
      <View style={{ flex: 1.1 }}>{thread}</View>
      <View style={rule} />
      <View style={{ flex: 1 }}>{stream}</View>
    </View>
  );
}
