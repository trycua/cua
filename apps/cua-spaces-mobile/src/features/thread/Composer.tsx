// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import * as Haptics from "expo-haptics";
import { useCallback, useRef, useState } from "react";
import { FlatList, Platform, Pressable, TextInput, View, type TextInputSelectionChangeEvent } from "react-native";

import { sendModeView } from "../../core/phone";
import { useTheme } from "../../theme/theme";
import { Avatar } from "../../ui/Avatar";
import { Icon } from "../../ui/Icon";
import { showMenu } from "../../ui/menu";
import { Text } from "../../ui/Text";
import { useDictation } from "./dictation";

export interface Mentionable {
  handle: string;
  name: string;
  kind: "person" | "agent";
  /** A status word for the picker ("asleep", "working"). */
  note: string | null;
}

export type SendHow = "send" | "queue" | "steer";

export interface ComposerProps {
  placeholder: string;
  mentionables: Mentionable[];
  /** An agent has a turn on its way: an empty box shows Stop, and Send becomes Queue (or Steer). */
  running: boolean;
  /** Messages already waiting behind the running turn ("2 messages queued"). */
  queued?: number;
  /** The Space's daemon can interrupt the running step (Steer). */
  canSteer?: boolean;
  /** The reader cannot post (a guest). */
  readOnly?: string | null;
  /** The message this one replies to. */
  replyingTo?: { author: string; text: string } | null;
  onCancelReply?(): void;
  onSend(text: string, how: SendHow): void;
  onStop(): void;
  /** "Use this screen": post with the Space's desktop as what the reader is looking at. */
  onSendWithScreen(text: string): void;
  initialText?: string;
}

const MAX_H = 132;

/** The mention being typed at the cursor, if any. */
export function mentionAt(text: string, cursor: number): { start: number; query: string } | null {
  const m = /(^|\s)@([a-z0-9._-]*)$/i.exec(text.slice(0, cursor));
  return m ? { start: cursor - m[2]!.length - 1, query: m[2]!.toLowerCase() } : null;
}

export function rankMentions(all: Mentionable[], query: string): Mentionable[] {
  return all
    .filter((m) => !query || m.handle.toLowerCase().startsWith(query) || m.name.toLowerCase().startsWith(query))
    .sort((a, b) => (a.kind === b.kind ? a.name.localeCompare(b.name) : a.kind === "agent" ? -1 : 1));
}

/**
 * The floating pill: a plus menu, the text, and one round button that is the microphone (empty box), Send, or, while an
 * agent runs, Queue (Steer on a long press or from the menu) and Stop. The words of Send, Queue and Steer and of every
 * dictation state are the app core's (`sendMode.view`, `voice.view`).
 */
export function Composer(p: ComposerProps) {
  const { c, mono } = useTheme();
  const [text, setText] = useState(p.initialText ?? "");
  const [sel, setSel] = useState({ start: 0, end: 0 });
  // Where the caret goes after the app (not the keyboard) changed the text: dictation, a picked mention.
  const [forced, setForced] = useState<{ start: number; end: number } | null>(null);
  const [height, setHeight] = useState(36);
  const input = useRef<React.ComponentRef<typeof TextInput>>(null);
  const ment = mentionAt(text, sel.start);
  const picks = ment ? rankMentions(p.mentionables, ment.query) : [];
  const hasText = text.trim().length > 0;
  const mode = sendModeView({ running: p.running, hasText, queued: p.queued ?? 0, canSteer: p.canSteer ?? false });

  // Dictation: the box as it was when the button went down, and where each guess lands.
  const box = useRef({ text, ...sel });
  box.current = { text, start: sel.start, end: sel.end };
  const readBox = useCallback(() => box.current, []);
  const writeBox = useCallback((t: string, cursor: number) => {
    setText(t);
    setSel({ start: cursor, end: cursor });
    setForced({ start: cursor, end: cursor });
  }, []);
  const voice = useDictation(readBox, writeBox);
  const notice = voice.view?.notice ?? null;

  const insert = useCallback(
    (m: Mentionable) => {
      Haptics.selectionAsync().catch(() => {});
      const at = mentionAt(text, sel.start);
      if (!at) return;
      const next = `${text.slice(0, at.start)}@${m.handle} ${text.slice(sel.start)}`;
      setText(next);
      const pos = at.start + m.handle.length + 2;
      setSel({ start: pos, end: pos });
      setForced({ start: pos, end: pos });
    },
    [text, sel.start],
  );
  const send = (how: SendHow) => {
    if (!hasText) return;
    Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Light).catch(() => {});
    p.onSend(text.trim(), how);
    setText("");
    setHeight(36);
  };

  if (p.readOnly) {
    return (
      <View style={{ margin: 12, padding: 14, borderRadius: 22, backgroundColor: c.card, borderWidth: 0.5, borderColor: c.border }}>
        <Text variant="subhead" color="muted" style={{ textAlign: "center" }}>
          {p.readOnly}
        </Text>
      </View>
    );
  }

  const stopping = !hasText && p.running;
  const micMain = !hasText && !p.running && voice.supported;
  const label = hasText
    ? (mode?.accessibilityLabel ?? "Send")
    : stopping
      ? (mode?.stopLabel ?? "Stop")
      : micMain
        ? (voice.view?.buttonLabel ?? "Dictate")
        : (mode?.accessibilityLabel ?? "Send, type a message first");
  const idle = !hasText && !stopping && !micMain;
  const primaryMode: SendHow = (mode?.primary.mode ?? "send") as SendHow;
  const queueing = hasText && p.running;
  const pill = queueing && mode ? mode.primary.label : null;

  const menu = () => {
    const items = [
      {
        label: "Mention someone",
        run: () => {
          setText((t) => (t && !t.endsWith(" ") ? `${t} @` : `${t}@`));
          input.current?.focus();
        },
      },
      {
        label: "Send with this screen",
        run: () => {
          if (hasText) {
            p.onSendWithScreen(text.trim());
            setText("");
            setHeight(36);
          }
        },
      },
    ];
    if (hasText && p.running && mode?.alternate) items.push({ label: `${mode.alternate.label}: ${mode.alternate.hint}`, run: () => send("steer") });
    showMenu(undefined, items);
  };

  const press = () => {
    if (hasText) send(primaryMode);
    else if (stopping) {
      Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Medium).catch(() => {});
      p.onStop();
    }
  };
  /** A long press on Queue offers the other choice. */
  const longPress = () => {
    if (!queueing || !mode?.alternate) return;
    Haptics.selectionAsync().catch(() => {});
    showMenu(undefined, [
      { label: `${mode.primary.label}: ${mode.primary.hint}`, run: () => send(mode.primary.mode as SendHow) },
      { label: `${mode.alternate.label}: ${mode.alternate.hint}`, run: () => send(mode.alternate!.mode as SendHow) },
    ]);
  };

  const micButton = (small: boolean) => (
    <Pressable
      onPressIn={voice.pressIn}
      onPressOut={voice.pressOut}
      accessibilityRole="button"
      accessibilityLabel={voice.listening ? "Stop dictating" : (voice.view?.buttonLabel ?? "Dictate")}
      accessibilityHint={voice.view?.buttonHint}
      accessibilityActions={[{ name: "activate", label: voice.listening ? "Stop dictating" : "Dictate" }]}
      onAccessibilityAction={() => voice.toggle()}
      accessibilityState={{ selected: voice.listening }}
      testID="composer-mic"
      style={{
        width: small ? 32 : 36,
        height: 36,
        borderRadius: 18,
        alignItems: "center",
        justifyContent: "center",
        backgroundColor: voice.listening ? c.destructive : small ? "transparent" : c.muted,
      }}
    >
      <Icon
        name={voice.listening ? "micFill" : "mic"}
        size={small ? 18 : 18}
        color={voice.listening ? c.primaryForeground : c.mutedForeground}
        weight="semibold"
      />
    </Pressable>
  );

  return (
    <View>
      {ment && picks.length ? (
        <View
          style={{
            marginHorizontal: 12,
            marginBottom: 6,
            borderRadius: 14,
            backgroundColor: c.card,
            borderWidth: 0.5,
            borderColor: c.border,
            maxHeight: 220,
            overflow: "hidden",
          }}
          accessibilityLabel="Mention someone"
        >
          <FlatList
            data={picks}
            keyboardShouldPersistTaps="always"
            keyExtractor={(m) => m.handle}
            renderItem={({ item }) => (
              <Pressable
                onPress={() => insert(item)}
                accessibilityRole="button"
                accessibilityLabel={`Mention ${item.name}${item.note ? `, ${item.note}` : ""}`}
                style={({ pressed }) => ({
                  flexDirection: "row",
                  alignItems: "center",
                  gap: 10,
                  paddingHorizontal: 12,
                  paddingVertical: 9,
                  backgroundColor: pressed ? c.accent : "transparent",
                })}
              >
                <Avatar name={item.name} kind={item.kind} size={28} />
                <Text weight="500" style={{ flex: 1 }}>
                  {item.name}
                </Text>
                <Text variant="footnote" color="muted" style={{ fontFamily: mono }}>
                  @{item.handle}
                </Text>
                {item.note ? (
                  <Text variant="footnote" color="muted">
                    {item.note}
                  </Text>
                ) : null}
              </Pressable>
            )}
          />
        </View>
      ) : null}
      {notice ? (
        <View
          style={{ marginHorizontal: 18, marginBottom: 6, flexDirection: "row", alignItems: "center", gap: 10 }}
          accessibilityLiveRegion="polite"
          testID="voice-notice"
        >
          <Text variant="footnote" color="muted" style={{ flex: 1 }}>
            {notice.text}
          </Text>
          {notice.action && notice.actionLabel ? (
            <Pressable onPress={() => voice.act(notice.action!)} accessibilityRole="button" hitSlop={8}>
              <Text variant="footnote" weight="600" color="brand">
                {notice.actionLabel}
              </Text>
            </Pressable>
          ) : null}
        </View>
      ) : null}
      {voice.listening ? (
        <View
          style={{ marginHorizontal: 18, marginBottom: 6, flexDirection: "row", alignItems: "center", gap: 8 }}
          accessibilityLiveRegion="polite"
          testID="voice-status"
        >
          <View style={{ width: 8, height: 8, borderRadius: 4, backgroundColor: c.destructive }} />
          <Text variant="footnote" weight="600">
            {voice.view?.status ?? "Listening…"}
          </Text>
          {voice.view?.privacyNote ? (
            <Text variant="footnote" color="muted" style={{ flex: 1 }} numberOfLines={1}>
              {voice.view.privacyNote}
            </Text>
          ) : null}
        </View>
      ) : null}
      {mode?.queuedLine ? (
        <Text variant="caption" color="muted" style={{ marginHorizontal: 18, marginBottom: 4 }} testID="queued-line">
          {mode.queuedLine}
        </Text>
      ) : null}
      {p.replyingTo ? (
        <View style={{ marginHorizontal: 18, marginBottom: 4, flexDirection: "row", alignItems: "center", gap: 8 }}>
          <View style={{ width: 2, alignSelf: "stretch", backgroundColor: c.brandStrong }} />
          <View style={{ flex: 1 }}>
            <Text variant="caption" weight="600">
              Replying to {p.replyingTo.author}
            </Text>
            <Text variant="caption" color="muted" numberOfLines={1}>
              {p.replyingTo.text}
            </Text>
          </View>
          <Pressable onPress={p.onCancelReply} accessibilityRole="button" accessibilityLabel="Cancel reply" hitSlop={8}>
            <Icon name="close" size={14} color={c.mutedForeground} />
          </Pressable>
        </View>
      ) : null}
      <View
        style={{
          marginHorizontal: 10,
          marginBottom: 8,
          minHeight: 48,
          borderRadius: 26,
          backgroundColor: c.card,
          borderWidth: 0.5,
          borderColor: c.border,
          flexDirection: "row",
          alignItems: "flex-end",
          paddingLeft: 6,
          paddingRight: 6,
          paddingVertical: 6,
          shadowColor: "#000",
          shadowOpacity: 0.08,
          shadowRadius: 10,
          shadowOffset: { width: 0, height: 2 },
          elevation: 2,
        }}
      >
        <Pressable
          onPress={menu}
          accessibilityRole="button"
          accessibilityLabel="Add to message"
          style={{ width: 36, height: 36, alignItems: "center", justifyContent: "center" }}
        >
          <Icon name="plus" size={20} color={c.mutedForeground} />
        </Pressable>
        <TextInput
          ref={input}
          value={text}
          onChangeText={setText}
          selection={forced ?? undefined}
          onSelectionChange={(e: TextInputSelectionChangeEvent) => {
            setSel(e.nativeEvent.selection);
            setForced(null);
          }}
          placeholder={p.placeholder}
          placeholderTextColor={c.mutedForeground}
          multiline
          onContentSizeChange={(e) => setHeight(Math.min(MAX_H, Math.max(36, e.nativeEvent.contentSize.height)))}
          style={{
            flex: 1,
            height: height,
            maxHeight: MAX_H,
            color: c.foreground,
            fontSize: 17,
            paddingTop: Platform.OS === "ios" ? 8 : 7,
            paddingBottom: Platform.OS === "ios" ? 8 : 7,
            paddingHorizontal: 6,
          }}
          accessibilityLabel={p.placeholder}
          returnKeyType="default"
          blurOnSubmit={false}
          testID="composer-input"
        />
        {/* The microphone stays the same button while the text fills in, so a finger held on it still ends the dictation. */}
        {voice.supported ? micButton(!micMain) : null}
        {micMain ? null : (
          <Pressable
            onPress={press}
            onLongPress={longPress}
            delayLongPress={350}
            disabled={idle}
            accessibilityRole="button"
            accessibilityLabel={label}
            accessibilityHint={queueing ? mode?.primary.hint : undefined}
            accessibilityActions={queueing && mode?.alternate ? [{ name: "steer", label: mode.alternate.label }] : undefined}
            onAccessibilityAction={(e) => {
              if (e.nativeEvent.actionName === "steer") send("steer");
            }}
            accessibilityState={{ disabled: idle }}
            testID="composer-send"
            style={{
              minWidth: 36,
              height: 36,
              borderRadius: 18,
              paddingHorizontal: pill ? 12 : 0,
              flexDirection: "row",
              gap: 5,
              alignItems: "center",
              justifyContent: "center",
              backgroundColor: idle ? c.muted : c.primary,
            }}
          >
            <Icon name={stopping ? "stop" : "send"} size={stopping ? 14 : 17} color={idle ? c.mutedForeground : c.primaryForeground} weight="bold" />
            {pill ? (
              <Text variant="footnote" weight="700" style={{ color: c.primaryForeground }}>
                {pill}
              </Text>
            ) : null}
          </Pressable>
        )}
      </View>
    </View>
  );
}
