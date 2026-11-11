// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useMemo, useState } from "react";
import { Alert, FlatList, Pressable, RefreshControl, TextInput, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { cacheLine } from "../../core/phone";
import type { AgentListRow } from "../../core/views";
import type { SpaceSummary } from "../../data/types";
import { useBackend, useSpaceList } from "../../state/backend";
import { useOutbox } from "../../state/stores";
import { useTheme } from "../../theme/theme";
import { Avatar } from "../../ui/Avatar";
import { Icon } from "../../ui/Icon";
import { showMenu } from "../../ui/menu";
import { now as clockNow } from "../../ui/clock";
import { Text } from "../../ui/Text";
import { ComputersSection, MachineChips, StoriesBar } from "./parts";
import { ConversationRow, SwipeRow } from "./rows";
import { ShareSheet } from "./ShareSheet";

export interface HomeProps {
  onOpen(spaceId: string): void;
  onSettings(): void;
  /** Start a task from the phone (the sheet over Home). */
  onNewTask?(): void;
}

const matches = (s: SpaceSummary, q: string) => {
  const t = q.trim().toLowerCase();
  if (!t) return true;
  return [s.name, s.machineName, s.kindLabel, ...s.members.map((m) => m.name)].some((x) => x.toLowerCase().includes(t));
};

/** One item of the list: an agent (the core's row), or a Space that has no agent yet. */
interface Item {
  key: string;
  row: AgentListRow | null;
  space: SpaceSummary;
}

function Skeleton() {
  const { c } = useTheme();
  return (
    <View style={{ paddingHorizontal: 16, gap: 18, paddingTop: 8 }} accessibilityLabel="Loading Spaces">
      {[0, 1, 2, 3].map((i) => (
        <View key={i} style={{ flexDirection: "row", gap: 12 }}>
          <View style={{ width: 52, height: 52, borderRadius: 26, backgroundColor: c.tile }} />
          <View style={{ flex: 1, gap: 8, paddingTop: 4 }}>
            <View style={{ height: 14, width: `${50 + (i % 2) * 15}%`, borderRadius: 7, backgroundColor: c.tile }} />
            <View style={{ height: 12, width: "85%", borderRadius: 6, backgroundColor: c.tile }} />
          </View>
        </View>
      ))}
    </View>
  );
}

/**
 * Home: a bar of live 16:9 thumbnails of the Spaces (a live dot, the name under each, an orange ring for Needs you)
 * above an iMessage style list of the agents you talk to, one status per row. One list of Spaces; the computers of your
 * account are a section below it with a state on each row, never rows of the list.
 */
export function HomeScreen({ onOpen, onSettings, onNewTask }: HomeProps) {
  const { c } = useTheme();
  const insets = useSafeAreaInsets();
  const backend = useBackend();
  const { status, list, error, refreshing, refresh, cached, syncedMs } = useSpaceList();
  const outbox = useOutbox();
  const waiting = outbox?.view(null);
  const [query, setQuery] = useState("");
  const [machine, setMachine] = useState<string | null>(null);
  const [sharing, setSharing] = useState<SpaceSummary | null>(null);
  const now = clockNow();

  const spaces = useMemo(() => (list?.spaces ?? []).filter((s) => (machine ? s.machineId === machine : true) && matches(s, query)), [list, machine, query]);
  const items = useMemo<Item[]>(() => {
    const byId = new Map(spaces.map((s) => [s.id, s]));
    const withAgents = new Set<string>();
    const out: Item[] = [];
    for (const r of list?.agents ?? []) {
      const s = byId.get(r.places[0]?.spaceId ?? "");
      if (!s) continue;
      withAgents.add(s.id);
      out.push({ key: r.key, row: r, space: s });
    }
    for (const s of spaces) if (!withAgents.has(s.id)) out.push({ key: `space:${s.id}`, row: null, space: s });
    return out;
  }, [list, spaces]);
  // Offline: what is shown was saved or last loaded, and the line says when (the core's words).
  const offlineLine = error && list ? (cacheLine(cached?.savedMs ?? syncedMs, now, true) ?? "You're offline.") : "";
  // The core's words around the lists: while it reads, and when there is nothing to show (never a computer faked as a Space).
  const reachingLine =
    status === "loading"
      ? "Loading Spaces…"
      : (list?.words?.reachingLine ?? (list?.incomplete ? "Some computers did not answer. Pull down to try again." : null));

  const actions = {
    onSleep: (s: SpaceSummary) => backend.sleep(s.id).then(refresh, (e) => Alert.alert("Could not put it to sleep", String(e?.message ?? e))),
    onShare: (s: SpaceSummary) => setSharing(s),
  };
  /** Long press a row: the same actions as the swipe, plus Open. What this backend cannot do is not listed. */
  const menuFor = (s: SpaceSummary) => () =>
    showMenu(s.name, [
      { label: "Open", run: () => onOpen(s.id) },
      ...(backend.capabilities.sleep && s.canSleep && s.tone !== "asleep" && s.machineOnline ? [{ label: "Sleep", run: () => actions.onSleep(s) }] : []),
      ...(backend.capabilities.share && s.canShare ? [{ label: "Share", run: () => actions.onShare(s) }] : []),
    ]);

  const header = (
    <View>
      <View style={{ paddingTop: insets.top + 6, paddingHorizontal: 16, flexDirection: "row", alignItems: "center" }}>
        <Text variant="largeTitle" style={{ flex: 1 }} accessibilityRole="header">
          Spaces
        </Text>
        {onNewTask && backend.capabilities.startTask ? (
          <Pressable
            onPress={onNewTask}
            accessibilityRole="button"
            accessibilityLabel="Start a task"
            testID="new-task"
            hitSlop={8}
            style={{ width: 34, height: 34, borderRadius: 17, backgroundColor: c.tile, alignItems: "center", justifyContent: "center", marginRight: 10 }}
          >
            <Icon name="compose" size={17} color={c.foreground} />
          </Pressable>
        ) : null}
        <Pressable onPress={onSettings} accessibilityRole="button" accessibilityLabel="Account and settings" hitSlop={8}>
          <Avatar name="You" kind="person" size={34} />
        </Pressable>
      </View>
      {backend.capabilities.sample ? (
        <View style={{ marginHorizontal: 16, marginTop: 4 }} accessibilityLabel="Sample data. Not your account.">
          <Text variant="footnote" color="warning">
            Sample data. Not your account.
          </Text>
        </View>
      ) : null}
      <View
        style={{
          marginHorizontal: 16,
          marginTop: 10,
          marginBottom: 10,
          minHeight: 38,
          borderRadius: 10,
          backgroundColor: c.tile,
          flexDirection: "row",
          alignItems: "center",
          paddingHorizontal: 10,
          gap: 8,
        }}
      >
        <Icon name="search" size={16} color={c.mutedForeground} />
        <TextInput
          value={query}
          onChangeText={setQuery}
          placeholder="Search Spaces, apps or agents"
          placeholderTextColor={c.mutedForeground}
          style={{ flex: 1, color: c.foreground, fontSize: 16, paddingVertical: 8 }}
          returnKeyType="search"
          autoCorrect={false}
          accessibilityLabel="Search Spaces, apps or agents"
          clearButtonMode="while-editing"
        />
      </View>
      {list && list.machines.length > 1 ? (
        <View style={{ marginBottom: 10 }}>
          <MachineChips machines={list.machines} value={machine} onChange={setMachine} />
        </View>
      ) : null}
      {error && list ? (
        <Pressable
          onPress={refresh}
          accessibilityRole="button"
          accessibilityLabel={`${offlineLine} Try again.`}
          style={{
            marginHorizontal: 16,
            marginBottom: 8,
            padding: 10,
            borderRadius: 10,
            backgroundColor: c.muted,
            flexDirection: "row",
            gap: 8,
            alignItems: "center",
          }}
          testID="home-offline"
        >
          <Icon name="reload" size={14} color={c.mutedForeground} />
          <Text variant="footnote" color="muted" style={{ flex: 1 }}>
            {offlineLine}
          </Text>
        </Pressable>
      ) : null}
      {waiting?.banner && waiting.rows.length ? (
        <Text variant="footnote" color="muted" style={{ marginHorizontal: 16, marginBottom: 8 }} accessibilityLiveRegion="polite" testID="home-outbox">
          {waiting.banner}
        </Text>
      ) : null}
      {reachingLine && status !== "loading" ? (
        <Text variant="footnote" color="muted" style={{ paddingHorizontal: 16, paddingBottom: 6 }} accessibilityLiveRegion="polite">
          {reachingLine}
        </Text>
      ) : null}
      {spaces.length ? (
        <View style={{ marginBottom: 6 }}>
          <StoriesBar spaces={spaces.filter((s) => s.machineOnline)} onOpen={onOpen} />
        </View>
      ) : null}
    </View>
  );

  if (status === "loading") {
    return (
      <View style={{ flex: 1, backgroundColor: c.background }}>
        {header}
        <Text variant="footnote" color="muted" style={{ paddingHorizontal: 16, paddingBottom: 12 }}>
          {reachingLine ?? "Loading Spaces…"}
        </Text>
        <Skeleton />
      </View>
    );
  }
  if (status === "error" && !list) {
    return (
      <View style={{ flex: 1, backgroundColor: c.background }}>
        {header}
        <View style={{ padding: 24, alignItems: "center", gap: 12 }}>
          <Text variant="headline">Could not reach your Spaces</Text>
          <Text variant="subhead" color="muted" style={{ textAlign: "center" }}>
            {error}
          </Text>
          <Pressable
            onPress={refresh}
            accessibilityRole="button"
            style={{ backgroundColor: c.primary, paddingHorizontal: 18, paddingVertical: 10, borderRadius: 10 }}
          >
            <Text weight="600" style={{ color: c.primaryForeground }}>
              Try again
            </Text>
          </Pressable>
        </View>
      </View>
    );
  }
  return (
    <View style={{ flex: 1, backgroundColor: c.background }}>
      <FlatList
        testID="home-list"
        data={items}
        keyExtractor={(i) => i.key}
        renderItem={({ item }) => (
          <SwipeRow space={item.space} caps={backend.capabilities} actions={actions}>
            <ConversationRow row={item.row} space={item.space} now={now} onPress={() => onOpen(item.space.id)} onLongPress={menuFor(item.space)} />
          </SwipeRow>
        )}
        ItemSeparatorComponent={() => <View style={{ height: 0.5, marginLeft: 80, backgroundColor: c.border }} />}
        ListHeaderComponent={header}
        ListFooterComponent={<ComputersSection computers={list?.computers ?? []} />}
        ListEmptyComponent={
          <View style={{ padding: 32, alignItems: "center", gap: 8 }}>
            <Text variant="headline">{query || machine ? "Nothing matches" : (list?.words?.empty?.title ?? "No Spaces yet")}</Text>
            <Text variant="subhead" color="muted" style={{ textAlign: "center" }}>
              {query || machine
                ? "Try another search or computer."
                : (list?.words?.empty?.body ?? "Make Spaces in Cua Spaces on a Mac or PC. This phone talks to them and watches.")}
            </Text>
          </View>
        }
        refreshControl={<RefreshControl refreshing={refreshing} onRefresh={refresh} tintColor={c.mutedForeground} />}
        keyboardShouldPersistTaps="handled"
        keyboardDismissMode="on-drag"
        contentContainerStyle={{ paddingBottom: insets.bottom + 24 }}
      />
      <ShareSheet space={sharing} onClose={() => setSharing(null)} />
    </View>
  );
}
