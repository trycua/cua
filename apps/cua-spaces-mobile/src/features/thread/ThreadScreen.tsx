// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { FlashList, type FlashListRef } from "@shopify/flash-list";
import * as Haptics from "expo-haptics";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Alert, Modal, NativeScrollEvent, NativeSyntheticEvent, Pressable, RefreshControl, ScrollView, View } from "react-native";
import { KeyboardAvoidingView } from "react-native-keyboard-controller";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { ensureUnlocked } from "../../auth/lock";
import { approvalsView, cacheLine, diffLabels, type PendingApproval } from "../../core/phone";
import type { AgentMember, SpaceSummary } from "../../data/types";
import { useBackend, useSpaceList, useThread, useToolRows } from "../../state/backend";
import { usePrefs } from "../../state/prefs";
import { useOutbox } from "../../state/stores";
import { useTheme } from "../../theme/theme";
import { Avatar, AvatarStack } from "../../ui/Avatar";
import { Icon } from "../../ui/Icon";
import { now as clockNow } from "../../ui/clock";
import { Text } from "../../ui/Text";
import { StatusBadge, toneColor } from "../home/parts";
import { StreamView } from "../stream/StreamView";
import { AuthorLine, Bubble, ChildSpaceRow, DayLine, LiveRow, PendingRow, SubagentRow, SystemNote, UnreadLine } from "./bubbles";
import { ExportCard, PlainApproval, ToolGroup, exportCardView } from "./cards";
import { Composer, type Mentionable, type SendHow } from "./Composer";
import { buildRows, type Row } from "./timeline";
import { Thumbnail } from "../../ui/Thumbnail";

export interface ThreadProps {
  spaceId: string;
  onBack?(): void;
  /** Open the full-screen stream (a route on phones). */
  onFullStream(): void;
  /** Wide layouts show the stream beside the thread and hide the header thumbnail. */
  hideHeaderStream?: boolean;
  initialStreamOpen?: boolean;
  initialDraft?: string;
  /** A thread entry to scroll to (a notification's deep link). */
  focusSeq?: number | null;
  /** Open the changes an agent made in this Space (the diff screen). */
  onOpenChanges?(): void;
}

/** What the status pill under the header says: one honest line about the room right now. */
export function pillFor(opts: {
  members: AgentMember[];
  live: { agent: string; stage: string }[];
  offline: boolean;
  hasOpenApproval: boolean;
  controlled: boolean;
}): { text: string; tone: "you" | "working" | "asleep" | "idle" | "failed" } {
  if (opts.offline) return { text: "Reconnecting…", tone: "idle" };
  if (opts.hasOpenApproval) return { text: "Needs you, approval", tone: "you" };
  const asking = opts.members.find((m) => (m.needsYou ?? []).length);
  if (asking) return { text: `Needs you, ${asking.identity?.displayName ?? asking.name} asked`, tone: "you" };
  if (opts.controlled) return { text: "You have control", tone: "working" };
  if (opts.live.length) return { text: `${opts.live[0]!.agent.replace(/^@/, "")} ${opts.live[0]!.stage}`, tone: "working" };
  const working = opts.members.find((m) => m.status === "working");
  if (working) return { text: `${working.identity?.displayName ?? working.name}${working.activity ? `, ${working.activity}` : ""}`, tone: "working" };
  if (opts.members.length && opts.members.every((m) => m.status === "asleep")) return { text: "Asleep, a message wakes it", tone: "asleep" };
  return { text: "Idle", tone: "idle" };
}

export function ThreadScreen({ spaceId, onBack, onFullStream, hideHeaderStream, initialStreamOpen, initialDraft, focusSeq, onOpenChanges }: ThreadProps) {
  const { c, reduceMotion } = useTheme();
  const insets = useSafeAreaInsets();
  const backend = useBackend();
  const thread = useThread(spaceId);
  const { list } = useSpaceList();
  const space: SpaceSummary | undefined = list?.spaces.find((s) => s.id === spaceId);
  const [streamOpen, setStreamOpen] = useState(!!initialStreamOpen);
  const [controlled, setControlled] = useState(false);
  const [members, setMembersOpen] = useState(false);
  const [replyTo, setReplyTo] = useState<{ seq: number; author: string; text: string } | null>(null);
  const tools = useToolRows(spaceId);
  const outbox = useOutbox();
  const [atBottom, setAtBottom] = useState(true);
  const [newCount, setNewCount] = useState(0);
  const listRef = useRef<FlashListRef<Row>>(null);
  const seen = useRef(0);

  const nameOf = useCallback((handle: string) => thread.members.find((m) => m.name === handle)?.identity?.displayName ?? handle, [thread.members]);
  const rows = useMemo(
    () => buildRows({ items: thread.items, me: thread.me, live: thread.live, pending: thread.pending, tools, now: Date.now(), agentName: nameOf }),
    [thread.items, thread.me, thread.live, thread.pending, tools, nameOf],
  );

  // Opening lands on the newest entry. Rows of different heights (a card) are measured after the first draw, so the
  // end is asked for again while the list settles, until the reader scrolls by hand.
  const touched = useRef(false);
  const landed = useRef(false);
  useEffect(() => {
    if (thread.status !== "ready" || landed.current || focusSeq != null || !rows.length) return;
    landed.current = true;
    for (const ms of [60, 200, 450, 900])
      setTimeout(() => {
        if (!touched.current && listRef.current) listRef.current.scrollToEnd({ animated: false });
      }, ms);
  }, [thread.status, rows.length, focusSeq]);

  // A notification's deep link: scroll to its entry once the thread has it.
  const focused = useRef<number | null>(null);
  useEffect(() => {
    if (focusSeq == null || focused.current === focusSeq || thread.status !== "ready") return;
    const i = rows.findIndex((r) => (r.type === "approval" || r.type === "message") && r.entry.seq === focusSeq);
    if (i >= 0) {
      focused.current = focusSeq;
      setTimeout(() => listRef.current?.scrollToIndex({ index: i, animated: !reduceMotion, viewPosition: 0.3 }), 120);
    }
  }, [focusSeq, rows, thread.status, reduceMotion]);

  // New rows while scrolled up are counted for the jump button; at the bottom the list follows by itself.
  useEffect(() => {
    if (atBottom) {
      seen.current = rows.length;
      setNewCount(0);
    } else setNewCount(Math.max(0, rows.length - seen.current));
  }, [rows.length, atBottom]);

  const openApproval = thread.items.some((i) => (i.type === "approval" || i.type === "join_request") && !i.answer);
  // Every approval waiting, as structured data: the core builds the buttons, "1 of 2" and the scope choices from it.
  const openApprovals = useMemo<PendingApproval[]>(
    () =>
      thread.items.flatMap((i) =>
        i.type === "approval" && !i.answer
          ? [
              {
                seq: i.entry.seq,
                kind: i.entry.approval?.kind ?? (i.entry.approval?.card ? "push" : ""),
                action: i.entry.approval?.action ?? "",
                scopeChoices: i.entry.approval?.scopeChoices ?? [],
              },
            ]
          : [],
      ),
    [thread.items],
  );
  /** An answer written down and not yet taken by the machine: the card says so and cannot be answered twice. */
  const waitingAnswer = (seq: number) => thread.pending.find((p) => p.kind === "answer" && outbox?.message(p.id)?.extra?.approval === seq) ?? null;
  const pill = pillFor({ members: thread.members, live: thread.live, offline: thread.offline, hasOpenApproval: openApproval, controlled });
  const asked = thread.members.find((m) => (m.needsYou ?? []).length && m.needsYou![0]!.kind === "question");
  const addressed = asked ?? thread.members.find((m) => m.status === "asleep" && thread.members.length === 1);
  const running = thread.live.length > 0 || thread.members.some((m) => m.status === "working");
  const mentionables: Mentionable[] = [
    ...thread.members.map<Mentionable>((m) => ({
      handle: m.name,
      name: m.identity?.displayName ?? m.name,
      kind: "agent",
      note: m.status === "asleep" ? "asleep" : m.status === "working" ? "working" : null,
    })),
    ...(space?.members
      .filter((m) => m.kind === "person")
      .map<Mentionable>((m) => ({ handle: m.name.toLowerCase().replace(/\s+/g, "-"), name: m.name, kind: "person", note: null })) ?? []),
  ];
  const guest = thread.role === "guest" || space?.role === "viewer";
  const defaultTo = thread.members.length === 1 ? thread.members[0]! : null;

  /** The agent that is working now (the one Steer interrupts). */
  const runningAgent = thread.live[0]?.agent.replace(/^@/, "") ?? thread.members.find((m) => m.status === "working")?.name ?? null;
  const send = useCallback(
    async (text: string, how: SendHow = "send", withScreen = false) => {
      let t = text;
      // One agent in the room and no mention: the message is for it (an asleep agent wakes on it).
      if (defaultTo && !/@[a-z0-9._-]+/i.test(t)) t = `@${defaultTo.name} ${t}`;
      await thread.post(t, {
        replyTo: replyTo?.seq ?? null,
        view: withScreen ? { kind: "desktop", app: null, title: null, windowId: null } : null,
        mode: how,
        agent: how !== "send" ? runningAgent : null,
      });
      setReplyTo(null);
      setTimeout(() => listRef.current?.scrollToEnd({ animated: !reduceMotion }), 60);
    },
    [defaultTo, thread, replyTo, reduceMotion, runningAgent],
  );

  const toggleControl = useCallback(
    async (on: boolean) => {
      setControlled(on);
      await backend.takeControl(spaceId, on).catch(() => setControlled(!on));
    },
    [backend, spaceId],
  );

  const onScroll = (e: NativeSyntheticEvent<NativeScrollEvent>) => {
    const { contentOffset, contentSize, layoutMeasurement } = e.nativeEvent;
    const dist = contentSize.height - (contentOffset.y + layoutMeasurement.height);
    setAtBottom(dist < 120);
  };

  const renderItem = ({ item }: { item: Row }) => {
    switch (item.type) {
      case "day":
        return <DayLine text={item.text} />;
      case "system":
        return <SystemNote text={item.text} />;
      case "unread":
        return <UnreadLine />;
      case "tools":
        return <ToolGroup calls={item.calls} />;
      case "live":
        return <LiveRow agent={item.agent} stage={item.stage} text={item.text} children={item.children} />;
      case "subagent":
        return <SubagentRow sub={item.sub} />;
      case "child_space":
        return <ChildSpaceRow space={item.space} />;
      case "pending":
        return <PendingRow row={item.row} onRetry={() => thread.retry(item.row.id)} onDiscard={() => thread.discard(item.row.id)} />;
      case "approval": {
        const view = exportCardView(item.entry.approval?.card ?? null);
        // An answer to an agent needs Face ID or the passcode if it is not recent: a taken phone cannot allow things.
        const answer = async (a: boolean, scope?: "once" | "space") => {
          if (!(await ensureUnlocked("Answer your agent", usePrefs.getState().requireUnlock && !backend.capabilities.sample))) return;
          return thread.answer(item.entry.seq, a, scope);
        };
        const approvals = item.answered
          ? null
          : approvalsView({ pending: openApprovals, current: item.entry.seq, allowLabel: view?.allow ?? null, denyLabel: view?.deny ?? null });
        const waiting = waitingAnswer(item.entry.seq);
        const again = { onRetry: waiting ? () => thread.retry(waiting.id) : undefined, onDiscard: waiting ? () => thread.discard(waiting.id) : undefined };
        return (
          <View>
            {item.answered ? null : (
              <View style={{ marginLeft: 12, marginTop: 10 }}>
                <AuthorLine author={item.entry.author} tsMs={item.entry.tsMs} />
              </View>
            )}
            {view ? (
              <ExportCard
                view={view}
                answered={item.answered}
                onAnswer={answer}
                fromName={nameOf(thread.members[0]?.name ?? "")}
                approvals={approvals}
                waiting={waiting}
                onOpenDiff={backend.capabilities.diff ? onOpenChanges : undefined}
                {...again}
              />
            ) : (
              <PlainApproval entry={item.entry} answered={item.answered} onAnswer={answer} approvals={approvals} waiting={waiting} {...again} />
            )}
          </View>
        );
      }
      case "creator":
        return (
          <View style={{ marginLeft: 12, marginTop: 10 }}>
            <AuthorLine author={item.entry.author} tsMs={item.entry.tsMs} />
          </View>
        );
      case "message":
        return (
          <Bubble
            entry={item.entry}
            mine={item.mine}
            showAuthor={item.showAuthor}
            quote={item.quote}
            onReply={(e) => setReplyTo({ seq: e.seq, author: e.author.name, text: e.text })}
            onOpenAttachment={(n) =>
              Alert.alert(n, "Files open in Cua Spaces on the computer that has them. Opening files on this phone is not available yet.")
            }
          />
        );
    }
  };

  const name = space?.name ?? "Space";
  const labels = diffLabels();
  // The header's two sides are as wide as each other so the Space's name stays centred.
  const side = onOpenChanges && backend.capabilities.diff ? 98 : 56;
  // Offline: what is shown is what was saved or last loaded, and the line says when. Messages waiting say so first.
  const banner = thread.offline ? (thread.outboxBanner ?? cacheLine(thread.cached?.savedMs ?? thread.syncedMs, clockNow(), true)) : thread.outboxBanner;
  const statusColor = toneColor(pill.tone, c);
  const placeholder = addressed ? `Answer ${addressed.identity?.displayName ?? addressed.name}` : `Message ${name}`;

  return (
    <KeyboardAvoidingView behavior="padding" style={{ flex: 1, backgroundColor: c.background }}>
      <View style={{ paddingTop: insets.top + 4, paddingBottom: 6, paddingHorizontal: 12 }}>
        <View style={{ flexDirection: "row", alignItems: "flex-start" }}>
          <View style={{ width: side, alignItems: "flex-start" }}>
            {onBack ? (
              <Pressable
                onPress={onBack}
                accessibilityRole="button"
                accessibilityLabel="Back to Spaces"
                style={{ width: 38, height: 38, borderRadius: 19, backgroundColor: c.tile, alignItems: "center", justifyContent: "center" }}
              >
                <Icon name="back" size={18} color={c.foreground} weight="semibold" />
              </Pressable>
            ) : null}
          </View>
          <View style={{ flex: 1, alignItems: "center", gap: 6 }}>
            {!hideHeaderStream && !streamOpen ? (
              <Pressable
                onPress={() => {
                  Haptics.selectionAsync().catch(() => {});
                  setStreamOpen(true);
                }}
                accessibilityRole="button"
                accessibilityLabel={`Open the live desktop of ${name}`}
                style={{ width: 112 }}
              >
                <Thumbnail scene={space?.scene ?? "blank"} live={space?.machineOnline ?? false} radius={7} seed={name.length} />
              </Pressable>
            ) : null}
            <View
              style={{
                backgroundColor: c.card,
                borderRadius: 14,
                paddingHorizontal: 12,
                paddingVertical: 5,
                alignItems: "center",
                borderWidth: 0.5,
                borderColor: c.border,
                maxWidth: "100%",
              }}
              accessible
              accessibilityLabel={`${name}. ${pill.text}`}
              accessibilityRole="header"
            >
              <Text variant="subhead" weight="600" numberOfLines={1}>
                {name}
              </Text>
              <Text variant="caption" numberOfLines={1} style={{ color: pill.tone === "idle" || pill.tone === "asleep" ? c.mutedForeground : statusColor }}>
                {pill.text}
              </Text>
            </View>
            {backend.capabilities.sample ? (
              <Text variant="caption" color="warning">
                Sample data
              </Text>
            ) : null}
          </View>
          <View style={{ width: side, flexDirection: "row", justifyContent: "flex-end", gap: 6 }}>
            {onOpenChanges && backend.capabilities.diff ? (
              <Pressable
                onPress={onOpenChanges}
                accessibilityRole="button"
                accessibilityLabel={labels?.open ?? "Open the full diff"}
                testID="open-changes"
                style={{ width: 38, height: 38, borderRadius: 19, backgroundColor: c.tile, alignItems: "center", justifyContent: "center" }}
              >
                <Icon name="diff" size={18} color={c.foreground} />
              </Pressable>
            ) : null}
            <Pressable
              onPress={() => setMembersOpen(true)}
              accessibilityRole="button"
              accessibilityLabel="Who is in this Space"
              style={{ borderRadius: 19, backgroundColor: c.tile, paddingHorizontal: 6, height: 38, alignItems: "center", justifyContent: "center" }}
            >
              <AvatarStack members={(space?.members ?? []).map((m) => ({ name: m.name, kind: m.kind }))} size={22} />
            </Pressable>
          </View>
        </View>
      </View>
      {streamOpen ? (
        <View style={{ marginBottom: 6 }}>
          <StreamView
            spaceId={spaceId}
            scene={space?.scene ?? "blank"}
            mode="compact"
            controlled={controlled}
            onClose={() => setStreamOpen(false)}
            onExpand={onFullStream}
            onToggleControl={toggleControl}
          />
        </View>
      ) : null}
      <View style={{ flex: 1 }}>
        {thread.status === "loading" ? (
          <View style={{ padding: 24, gap: 12 }} accessibilityLabel="Loading messages">
            {[0.6, 0.8, 0.5].map((w, i) => (
              <View
                key={i}
                style={{ height: 38, width: `${w * 100}%`, borderRadius: 18, backgroundColor: c.tile, alignSelf: i % 2 ? "flex-end" : "flex-start" }}
              />
            ))}
          </View>
        ) : thread.status === "error" ? (
          <View style={{ padding: 24, alignItems: "center", gap: 10 }}>
            <Text variant="headline">Could not open this thread</Text>
            <Text color="muted" style={{ textAlign: "center" }}>
              {thread.error}
            </Text>
            <Pressable
              onPress={thread.refresh}
              accessibilityRole="button"
              style={{ backgroundColor: c.primary, borderRadius: 10, paddingHorizontal: 16, paddingVertical: 9 }}
            >
              <Text weight="600" style={{ color: c.primaryForeground }}>
                Try again
              </Text>
            </Pressable>
          </View>
        ) : (
          <FlashList
            ref={listRef}
            data={rows}
            renderItem={renderItem}
            keyExtractor={(r) => r.key}
            getItemType={(r) => r.type}
            maintainVisibleContentPosition={{ autoscrollToBottomThreshold: 0.25, startRenderingFromBottom: true, animateAutoScrollToBottom: !reduceMotion }}
            onScroll={onScroll}
            onScrollBeginDrag={() => {
              touched.current = true;
            }}
            scrollEventThrottle={64}
            keyboardShouldPersistTaps="handled"
            keyboardDismissMode="interactive"
            onStartReached={thread.hasMoreBefore ? thread.loadEarlier : undefined}
            refreshControl={<RefreshControl refreshing={false} onRefresh={thread.refresh} tintColor={c.mutedForeground} />}
            contentContainerStyle={{ paddingBottom: 8 }}
            ListEmptyComponent={
              <View style={{ padding: 32, alignItems: "center", gap: 6 }}>
                <Text variant="headline">No messages yet</Text>
                <Text color="muted" style={{ textAlign: "center" }}>
                  Start with an @mention to give an agent a task.
                </Text>
              </View>
            }
          />
        )}
        {banner ? (
          <View
            style={{ position: "absolute", top: 0, left: 0, right: 0, backgroundColor: c.muted, paddingVertical: 4 }}
            accessibilityLiveRegion="polite"
            testID="thread-banner"
          >
            <Text variant="caption" color="muted" style={{ textAlign: "center" }}>
              {banner}
            </Text>
          </View>
        ) : null}
        {!atBottom ? (
          <Pressable
            onPress={() => {
              Haptics.selectionAsync().catch(() => {});
              listRef.current?.scrollToEnd({ animated: !reduceMotion });
            }}
            accessibilityRole="button"
            accessibilityLabel={newCount ? `Jump to latest, ${newCount} new` : "Jump to latest"}
            style={{
              position: "absolute",
              right: 14,
              bottom: 10,
              height: 38,
              minWidth: 38,
              borderRadius: 19,
              paddingHorizontal: newCount ? 12 : 0,
              flexDirection: "row",
              gap: 6,
              alignItems: "center",
              justifyContent: "center",
              backgroundColor: c.card,
              borderWidth: 0.5,
              borderColor: c.border,
              shadowColor: "#000",
              shadowOpacity: 0.12,
              shadowRadius: 6,
              shadowOffset: { width: 0, height: 2 },
            }}
          >
            <Icon name="down" size={16} color={c.foreground} />
            {newCount ? (
              <Text variant="footnote" weight="600">
                {newCount} new
              </Text>
            ) : null}
          </Pressable>
        ) : null}
      </View>
      <View style={{ paddingBottom: insets.bottom > 0 ? 0 : 4 }}>
        <Composer
          key={spaceId}
          initialText={initialDraft}
          placeholder={placeholder}
          mentionables={mentionables}
          running={running}
          queued={thread.members.reduce((n, m) => n + (m.queued ?? 0), 0)}
          canSteer={backend.capabilities.steer}
          readOnly={guest ? "You can watch this Space. Ask the owner for edit access to message it." : null}
          replyingTo={replyTo}
          onCancelReply={() => setReplyTo(null)}
          onSend={(t, how) => send(t, how)}
          onSendWithScreen={(t) => send(t, "send", true)}
          onStop={() => backend.stopAgent(spaceId, thread.live[0]?.agent.replace(/^@/, "") ?? thread.members.find((m) => m.status === "working")?.name ?? "")}
        />
      </View>
      <MembersSheet open={members} onClose={() => setMembersOpen(false)} space={space} members={thread.members} />
    </KeyboardAvoidingView>
  );
}

function MembersSheet({ open, onClose, space, members }: { open: boolean; onClose(): void; space?: SpaceSummary; members: AgentMember[] }) {
  const { c } = useTheme();
  const insets = useSafeAreaInsets();
  const people = space?.members.filter((m) => m.kind === "person") ?? [];
  const word = (s: string) => (s === "working" ? "Working" : s === "asleep" ? "Asleep" : s === "waiting" ? "Needs you" : s === "paused" ? "Paused" : "Idle");
  const tone = (s: string) =>
    (s === "working" ? "working" : s === "asleep" ? "asleep" : s === "waiting" ? "you" : "idle") as "working" | "asleep" | "you" | "idle";
  const section = (t: string) => (
    <Text variant="footnote" color="muted" weight="600" style={{ marginTop: 14, marginBottom: 6, textTransform: "uppercase", letterSpacing: 0.4 }}>
      {t}
    </Text>
  );
  return (
    <Modal visible={open} transparent animationType="slide" onRequestClose={onClose}>
      <Pressable style={{ flex: 1, backgroundColor: c.overlay }} onPress={onClose} accessibilityLabel="Close" />
      <View style={{ backgroundColor: c.card, borderTopLeftRadius: 16, borderTopRightRadius: 16, maxHeight: "70%", paddingBottom: insets.bottom + 12 }}>
        <View style={{ flexDirection: "row", alignItems: "center", padding: 16, paddingBottom: 0 }}>
          <Text variant="headline" style={{ flex: 1 }}>
            In this Space
          </Text>
          <Pressable onPress={onClose} accessibilityRole="button" accessibilityLabel="Done">
            <Text weight="600" color="brand">
              Done
            </Text>
          </Pressable>
        </View>
        <ScrollView contentContainerStyle={{ paddingHorizontal: 16, paddingBottom: 8 }}>
          {section("People")}
          {people.map((m) => (
            <View
              key={m.id}
              style={{ flexDirection: "row", alignItems: "center", gap: 12, paddingVertical: 8 }}
              accessible
              accessibilityLabel={`${m.name}, ${m.role ?? ""}, ${m.here ? "here now" : "away"}`}
            >
              <Avatar name={m.name} kind="person" size={36} dot={m.here ? c.success : null} />
              <View style={{ flex: 1 }}>
                <Text weight="500">{m.name}</Text>
                <Text variant="footnote" color="muted">
                  {m.role}
                  {m.here ? ", here now" : ""}
                </Text>
              </View>
            </View>
          ))}
          {section("Agents")}
          {members.map((m) => (
            <View
              key={m.name}
              style={{ flexDirection: "row", alignItems: "center", gap: 12, paddingVertical: 8 }}
              accessible
              accessibilityLabel={`${m.identity?.displayName ?? m.name}, ${word(m.status)}`}
            >
              <Avatar name={m.name} kind="agent" size={36} />
              <View style={{ flex: 1 }}>
                <Text weight="500">{m.identity?.displayName ?? m.name}</Text>
                <Text variant="footnote" color="muted">
                  {m.activity ?? m.harness}
                </Text>
              </View>
              <StatusBadge tone={tone(m.status)} word={word(m.status)} />
            </View>
          ))}
        </ScrollView>
      </View>
    </Modal>
  );
}
