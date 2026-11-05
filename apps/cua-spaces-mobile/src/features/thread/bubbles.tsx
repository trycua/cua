// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import * as Clipboard from "expo-clipboard";
import { Pressable, View } from "react-native";
import Animated, { FadeIn } from "react-native-reanimated";

import type { OutboxRow } from "../../core/phone";
import type { ThreadEntry } from "../../data/types";
import { useTheme } from "../../theme/theme";
import { Avatar } from "../../ui/Avatar";
import { Icon } from "../../ui/Icon";
import { showMenu } from "../../ui/menu";
import { Text } from "../../ui/Text";
import { clock } from "../../ui/time";
import { Markdown } from "./markdown";
import { authorView } from "./timeline";

/** The text with @mentions in Cua Blue. */
export function MentionText({ text, color }: { text: string; color: string }) {
  const { c, scheme } = useTheme();
  const parts = text.split(/(@[a-z0-9._-]+)/gi);
  return (
    <Text variant="body" style={{ color }}>
      {parts.map((p, i) => (p.startsWith("@") ? <Text key={i} variant="body" weight="600" style={{ color: scheme === "dark" ? c.brand : c.brandStrong }}>{p}</Text> : p))}
    </Text>
  );
}

export function DayLine({ text }: { text: string }) {
  return (
    <Text variant="caption" color="muted" style={{ textAlign: "center", marginTop: 14, marginBottom: 6 }}>{text}</Text>
  );
}

export function SystemNote({ text }: { text: string }) {
  return <Text variant="footnote" color="muted" style={{ textAlign: "center", marginVertical: 6, paddingHorizontal: 32 }}>{text}</Text>;
}

export function UnreadLine() {
  const { c } = useTheme();
  return (
    <View style={{ flexDirection: "row", alignItems: "center", gap: 10, marginHorizontal: 16, marginVertical: 10 }} accessible accessibilityLabel="Since you were here">
      <View style={{ flex: 1, height: 0.5, backgroundColor: c.warning }} />
      <Text variant="caption" weight="600" color="warning">Since you were here</Text>
      <View style={{ flex: 1, height: 0.5, backgroundColor: c.warning }} />
    </View>
  );
}

/** Who wrote it: the avatar (Cua's own entries use the app icon), the name, the APP tag, the time. */
export function AuthorLine({ author, tsMs, inset = 2 }: { author: { kind: string; id: string; name: string }; tsMs: number; inset?: number }) {
  const { c } = useTheme();
  const a = authorView(author);
  return (
    <View style={{ flexDirection: "row", alignItems: "center", gap: 6, marginBottom: 4, marginLeft: inset }} accessible accessibilityLabel={`${a.name}${a.tag ? `, ${a.tag}` : ""}, ${clock(tsMs)}`}>
      <Avatar name={a.name} kind={a.kind} size={20} />
      <Text variant="caption" weight="600">{a.name}</Text>
      {a.tag ? (
        <View style={{ backgroundColor: c.muted, borderRadius: 4, paddingHorizontal: 5, paddingVertical: 1 }}>
          <Text variant="caption" weight="700" color="muted" style={{ fontSize: 10, lineHeight: 13 }}>{a.tag}</Text>
        </View>
      ) : null}
      <Text variant="caption" color="muted">{clock(tsMs)}</Text>
    </View>
  );
}

export interface BubbleProps {
  entry: ThreadEntry;
  mine: boolean;
  showAuthor: boolean;
  quote: { author: string; text: string } | null;
  onReply(entry: ThreadEntry): void;
  onOpenAttachment(name: string): void;
}

export function Bubble({ entry, mine, showAuthor, quote, onReply, onOpenAttachment }: BubbleProps) {
  const { c } = useTheme();
  const a = authorView(entry.author);
  const textColor = mine ? c.bubbleMineText : c.foreground;
  const menu = () =>
    showMenu(undefined, [
      { label: "Reply", run: () => onReply(entry) },
      { label: "Copy", run: () => void Clipboard.setStringAsync(entry.text) },
    ]);
  return (
    <View style={{ paddingHorizontal: 12, marginTop: showAuthor ? 10 : 2, alignItems: mine ? "flex-end" : "flex-start" }}>
      {showAuthor ? <AuthorLine author={entry.author} tsMs={entry.tsMs} /> : null}
      <Pressable
        onLongPress={menu}
        delayLongPress={350}
        accessibilityRole="text"
        accessibilityLabel={`${mine ? "You" : a.name}: ${entry.text}`}
        accessibilityHint="Double tap and hold for actions"
        style={{ maxWidth: "82%", backgroundColor: mine ? c.bubbleMine : c.bubbleOther, borderRadius: 18, paddingHorizontal: 13, paddingVertical: 8, borderBottomLeftRadius: !mine && showAuthor ? 6 : 18 }}
      >
        {quote ? (
          <View style={{ borderLeftWidth: 2, borderLeftColor: mine ? c.bubbleMineText : c.mutedForeground, paddingLeft: 8, marginBottom: 6, opacity: 0.8 }}>
            <Text variant="caption" weight="600" style={{ color: textColor }}>{quote.author}</Text>
            <Text variant="caption" numberOfLines={2} style={{ color: textColor }}>{quote.text}</Text>
          </View>
        ) : null}
        {mine ? <MentionText text={entry.text} color={textColor} /> : <Markdown text={entry.text} color={textColor} />}
      </Pressable>
      {entry.attachments.length ? (
        <View style={{ flexDirection: "row", flexWrap: "wrap", gap: 6, marginTop: 6 }}>
          {entry.attachments.map((f) => (
            <Pressable key={f.path} onPress={() => onOpenAttachment(f.name)} accessibilityRole="button" accessibilityLabel={`${f.name}${f.size ? `, ${f.size} bytes` : ""}`} style={{ flexDirection: "row", alignItems: "center", gap: 6, backgroundColor: c.tile, borderRadius: 14, paddingHorizontal: 10, paddingVertical: 6 }}>
              <Icon name="doc" size={13} color={c.mutedForeground} />
              <Text variant="footnote">{f.name}</Text>
            </Pressable>
          ))}
        </View>
      ) : null}
    </View>
  );
}

/** What an agent is doing now: its words so far while it streams, or one quiet working line. */
export function LiveRow({ agent, stage, text }: { agent: string; stage: string; text: string }) {
  const { c, reduceMotion } = useTheme();
  const Wrap = reduceMotion ? View : Animated.View;
  return (
    <Wrap {...(reduceMotion ? {} : { entering: FadeIn.duration(180) })} style={{ paddingHorizontal: 12, marginTop: 10 }} accessibilityLiveRegion="polite">
      <View style={{ flexDirection: "row", alignItems: "center", gap: 6, marginBottom: 4, marginLeft: 2 }}>
        <Avatar name={agent} kind="agent" size={20} />
        <Text variant="caption" weight="600">{agent}</Text>
        <Text variant="caption" color="success">{stage}</Text>
      </View>
      {text ? (
        <View style={{ maxWidth: "82%", alignSelf: "flex-start", backgroundColor: c.bubbleOther, borderRadius: 18, borderBottomLeftRadius: 6, paddingHorizontal: 13, paddingVertical: 8 }}>
          <Markdown text={text} color={c.foreground} />
        </View>
      ) : null}
    </Wrap>
  );
}

/**
 * A message the machine has not taken yet, as the person's own words with the core's line under it ("Waiting for a
 * connection", "Sending…", "Not sent"). Retry and Delete are there when they mean something; nothing is claimed sent.
 */
export function PendingRow({ row, onRetry, onDiscard }: { row: OutboxRow; onRetry(): void; onDiscard(): void }) {
  const { c } = useTheme();
  const failed = row.state === "failed";
  return (
    <View style={{ paddingHorizontal: 12, marginTop: 6, alignItems: "flex-end" }} testID={`pending-${row.state}`} accessible accessibilityLabel={`${row.text}. ${row.label}`}>
      <View style={{ maxWidth: "82%", backgroundColor: c.bubbleMine, opacity: failed ? 0.55 : 0.7, borderRadius: 18, paddingHorizontal: 13, paddingVertical: 8 }}>
        <MentionText text={row.text} color={c.bubbleMineText} />
      </View>
      <View style={{ flexDirection: "row", gap: 14, marginTop: 4 }}>
        <Text variant="caption" color={failed ? "destructive" : "muted"}>{row.label}</Text>
        {row.canRetry ? <Pressable onPress={onRetry} accessibilityRole="button" accessibilityLabel={`${row.retryLabel} sending`} hitSlop={8}><Text variant="caption" weight="600" color="brand">{row.retryLabel}</Text></Pressable> : null}
        {row.canDiscard ? <Pressable onPress={onDiscard} accessibilityRole="button" accessibilityLabel={`${row.discardLabel} this message`} hitSlop={8}><Text variant="caption" color="muted">{row.discardLabel}</Text></Pressable> : null}
      </View>
    </View>
  );
}
