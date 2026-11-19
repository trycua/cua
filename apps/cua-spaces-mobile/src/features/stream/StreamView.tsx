// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import * as Haptics from "expo-haptics";
import { useCallback, useEffect, useRef, useState } from "react";
import { ActivityIndicator, Pressable, TextInput, View, type LayoutChangeEvent } from "react-native";
import { Gesture, GestureDetector } from "react-native-gesture-handler";
import Animated, { useAnimatedStyle, useSharedValue } from "react-native-reanimated";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { WebView } from "react-native-webview";

import { ControlNotAllowedError, type Scene, type StreamTicket } from "../../data/types";
import { useBackend } from "../../state/backend";
import { useTheme } from "../../theme/theme";
import { Icon } from "../../ui/Icon";
import { Text } from "../../ui/Text";
import { Thumbnail } from "../../ui/Thumbnail";

export interface StreamProps {
  spaceId: string;
  scene: Scene;
  mode: "compact" | "full";
  /** In a pane beside the thread (no close button, no status bar clearance). */
  embedded?: boolean;
  controlled: boolean;
  onClose(): void;
  onExpand?(): void;
  onToggleControl(on: boolean): void;
}

type Pt = { x: number; y: number };

/** What the surface shows: the daemon's HTML5 viewer in a web view when a ticket exists; the sample scene for sample data; else why not. */
function Surface({ ticket, scene, loading, error }: { ticket: StreamTicket | null; scene: Scene; loading: boolean; error: string | null }) {
  const backend = useBackend();
  if (loading)
    return (
      <View style={{ flex: 1, alignItems: "center", justifyContent: "center", backgroundColor: "#0b0c0f" }}>
        <ActivityIndicator color="#fff" />
      </View>
    );
  if (backend.capabilities.sample)
    return (
      <View style={{ flex: 1 }}>
        <Thumbnail scene={scene} radius={0} style={{ aspectRatio: undefined, flex: 1, borderWidth: 0 }} />
        <View style={{ position: "absolute", left: 8, bottom: 8, backgroundColor: "#000a", borderRadius: 6, paddingHorizontal: 6, paddingVertical: 2 }}>
          <Text variant="caption" style={{ color: "#fff" }}>
            Sample screen
          </Text>
        </View>
      </View>
    );
  if (!ticket) {
    return (
      <View style={{ flex: 1, alignItems: "center", justifyContent: "center", backgroundColor: "#0b0c0f", padding: 16 }}>
        <Text variant="subhead" style={{ color: "#fff", textAlign: "center" }}>
          {error ?? backend.capabilities.why.stream ?? "The live screen is not available yet."}
        </Text>
      </View>
    );
  }
  const origin = originOf(ticket.viewerUrl);
  return (
    <WebView
      // The address and nothing else: the page never holds the account's bearer or this phone's session, only the ticket in the
      // address's fragment, and it can go nowhere but the relay it came from.
      source={{ uri: ticket.viewerUrl }}
      pointerEvents="none"
      style={{ flex: 1, backgroundColor: "#0b0c0f" }}
      originWhitelist={origin ? [`${origin}/*`] : []}
      onShouldStartLoadWithRequest={(request) => allowedNavigation(origin, request.url)}
      javaScriptCanOpenWindowsAutomatically={false}
      setSupportMultipleWindows={false}
      sharedCookiesEnabled={false}
      thirdPartyCookiesEnabled={false}
      incognito
      cacheEnabled={false}
      allowsInlineMediaPlayback
      mediaPlaybackRequiresUserAction={false}
      javaScriptEnabled
      accessibilityLabel="Live desktop"
    />
  );
}

/** `https://relay.example` of an address, or null. */
export function originOf(url: string): string | null {
  const m = /^(https:\/\/[^/?#]+)/i.exec(url);
  return m ? m[1]!.toLowerCase() : null;
}

/** Whether the live-desktop page may load `url`: the viewer under the relay it started from, over https, and nothing else. */
export function allowedNavigation(origin: string | null, url: string): boolean {
  if (!origin) return false;
  if (url === "about:blank") return true;
  return originOf(url) === origin && /^https:\/\/[^/?#]+\/m\/[^/?#]+\/viewer(\/|$|\?|#)/i.test(url);
}

const KEYS = [
  ["Esc", "escape"],
  ["Tab", "tab"],
  ["Ctrl", "ctrl"],
  ["Alt", "alt"],
  ["Cmd", "cmd"],
  ["Left", "left"],
  ["Up", "up"],
  ["Down", "down"],
  ["Right", "right"],
  ["Enter", "return"],
] as const;

/** The live desktop of a Space: compact in the thread, full screen with touch controls. */
export function StreamView({ spaceId, scene, mode, embedded, controlled, onClose, onExpand, onToggleControl }: StreamProps) {
  const insets = useSafeAreaInsets();
  const { c, reduceMotion } = useTheme();
  const backend = useBackend();
  const [ticket, setTicket] = useState<StreamTicket | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [size, setSize] = useState({ w: 1, h: 1 });
  const [trackpad, setTrackpad] = useState(false);
  const [keys, setKeys] = useState(false);
  const [typing, setTyping] = useState(false);
  const [cursor, setCursor] = useState<Pt>({ x: 0.5, y: 0.5 });
  const [mods, setMods] = useState<string[]>([]);
  const scale = useSharedValue(1);
  const tx = useSharedValue(0);
  const ty = useSharedValue(0);
  const base = useRef({ scale: 1, tx: 0, ty: 0 });

  useEffect(() => {
    let alive = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const load = (first: boolean) => {
      if (first) setLoading(true);
      backend.streamTicket(spaceId).then(
        (t) => {
          if (!alive) return;
          setTicket(t);
          setLoading(false);
          // A ticket is short. The page is loaded again with a new one a minute before this one ends.
          if (t) timer = setTimeout(() => load(false), Math.max(30_000, t.expiresAtMs - Date.now() - 60_000));
        },
        (e) => {
          if (!alive) return;
          setError(e instanceof Error ? e.message : String(e));
          setLoading(false);
        },
      );
    };
    load(true);
    return () => {
      alive = false;
      if (timer) clearTimeout(timer);
    };
  }, [backend, spaceId]);

  const toDesktop = useCallback(
    (p: Pt) => {
      // The view is scaled about its centre and moved by (tx, ty); the answer is 0..1 across the desktop.
      const nx = (p.x - size.w / 2 - base.current.tx) / base.current.scale + size.w / 2;
      const ny = (p.y - size.h / 2 - base.current.ty) / base.current.scale + size.h / 2;
      return { x: Math.min(1, Math.max(0, nx / size.w)), y: Math.min(1, Math.max(0, ny / size.h)) };
    },
    [size],
  );
  const [controlNote, setControlNote] = useState<string | null>(null);
  const send = useCallback(
    (a: Parameters<typeof backend.sendInput>[1]) =>
      backend.sendInput(spaceId, a).then(
        () => setControlNote(null),
        (e) => {
          // Waiting for the person at the computer is a state with words; any other failure stays quiet as before.
          if (e instanceof ControlNotAllowedError) setControlNote("You can only look at this desktop. Its owner can let you control it in the share.");
        },
      ),
    [backend, spaceId],
  );
  const at = (p: Pt) => (trackpad ? { x: cursor.x, y: cursor.y } : toDesktop(p));

  const full = mode === "full";
  const enabled = full && controlled;
  // A tap clicks at once. There is no double-tap gesture to wait for: two quick taps are two clicks, and the desktop
  // reads them as a double click.
  const tap = Gesture.Tap()
    .runOnJS(true)
    .enabled(enabled)
    .onEnd((e, ok) => {
      if (ok) {
        Haptics.selectionAsync().catch(() => {});
        send({ type: "click", ...at({ x: e.x, y: e.y }) });
      }
    });
  const press = Gesture.LongPress()
    .runOnJS(true)
    .minDuration(450)
    .enabled(enabled)
    .onStart((e) => {
      Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Medium).catch(() => {});
      send({ type: "click", ...at({ x: e.x, y: e.y }), button: "right" });
    });
  const scroll = Gesture.Pan()
    .runOnJS(true)
    .minPointers(2)
    .maxPointers(2)
    .enabled(enabled)
    .onChange((e) => {
      send({ type: "scroll", ...at({ x: e.x, y: e.y }), dx: Math.round(-e.changeX), dy: Math.round(-e.changeY) });
    });
  const move = Gesture.Pan()
    .runOnJS(true)
    .maxPointers(1)
    .enabled(full && (trackpad || scale.value > 1))
    .onChange((e) => {
      if (trackpad)
        setCursor((p) => ({ x: Math.min(1, Math.max(0, p.x + (e.changeX * 1.6) / size.w)), y: Math.min(1, Math.max(0, p.y + (e.changeY * 1.6) / size.h)) }));
      else {
        tx.value += e.changeX;
        ty.value += e.changeY;
        base.current.tx = tx.value;
        base.current.ty = ty.value;
      }
    });
  const pinch = Gesture.Pinch()
    .runOnJS(true)
    .enabled(full)
    .onUpdate((e) => {
      scale.value = Math.min(4, Math.max(1, base.current.scale * e.scale));
    })
    .onEnd(() => {
      base.current.scale = scale.value;
      if (scale.value <= 1.01) {
        scale.value = 1;
        tx.value = 0;
        ty.value = 0;
        base.current = { scale: 1, tx: 0, ty: 0 };
      }
    });
  const gestures = Gesture.Simultaneous(pinch, scroll, move, Gesture.Exclusive(press, tap));
  const zoomStyle = useAnimatedStyle(() => ({ transform: [{ translateX: tx.value }, { translateY: ty.value }, { scale: scale.value }] }));
  const hiddenInput = useRef<React.ComponentRef<typeof TextInput>>(null);

  const bar = (children: React.ReactNode) => <View style={{ flexDirection: "row", alignItems: "center", gap: 8 }}>{children}</View>;
  const round = (name: Parameters<typeof Icon>[0]["name"], label: string, onPress: () => void, on = false) => (
    <Pressable
      onPress={onPress}
      accessibilityRole="button"
      accessibilityLabel={label}
      accessibilityState={{ selected: on }}
      hitSlop={6}
      style={{ width: 38, height: 38, borderRadius: 19, alignItems: "center", justifyContent: "center", backgroundColor: on ? "#ffffff" : "#ffffff30" }}
    >
      <Icon name={name} size={17} color={on ? "#111" : "#fff"} />
    </Pressable>
  );

  if (!full) {
    return (
      <View
        style={{ marginHorizontal: 12, borderRadius: 12, overflow: "hidden", aspectRatio: 16 / 9, backgroundColor: "#0b0c0f" }}
        onLayout={(e: LayoutChangeEvent) => setSize({ w: e.nativeEvent.layout.width, h: e.nativeEvent.layout.height })}
      >
        <Surface ticket={ticket} scene={scene} loading={loading} error={error} />
        <Pressable
          onPress={onExpand}
          accessibilityRole="button"
          accessibilityLabel="Open the live desktop full screen"
          style={{ position: "absolute", inset: 0 }}
        />
        <View style={{ position: "absolute", top: 8, right: 8, flexDirection: "row", gap: 8 }}>
          {round("expand", "Full screen", () => onExpand?.())}
          {round("collapse", "Fold back into the header", onClose)}
        </View>
      </View>
    );
  }
  return (
    <View style={{ flex: 1, backgroundColor: "#000" }}>
      <View style={{ flex: 1, justifyContent: "center" }}>
        <GestureDetector gesture={gestures}>
          <View
            collapsable={false}
            style={{ width: "100%", aspectRatio: (ticket?.width ?? 16) / (ticket?.height ?? 9), maxHeight: "100%", alignSelf: "center", overflow: "hidden" }}
            onLayout={(e) => setSize({ w: e.nativeEvent.layout.width, h: e.nativeEvent.layout.height })}
            accessibilityLabel={controlled ? "Desktop. Tap to click, hold to right-click, two fingers to scroll, pinch to zoom." : "Desktop, watching"}
          >
            <Animated.View style={[{ flex: 1 }, reduceMotion ? { transform: [] } : null, zoomStyle]}>
              <Surface ticket={ticket} scene={scene} loading={loading} error={error} />
              {trackpad ? (
                <View
                  pointerEvents="none"
                  style={{
                    position: "absolute",
                    left: `${cursor.x * 100}%`,
                    top: `${cursor.y * 100}%`,
                    width: 14,
                    height: 14,
                    marginLeft: -7,
                    marginTop: -7,
                    borderRadius: 7,
                    borderWidth: 2,
                    borderColor: "#fff",
                    backgroundColor: "#00000055",
                  }}
                />
              ) : null}
            </Animated.View>
          </View>
        </GestureDetector>
        {!controlled ? (
          <View
            pointerEvents="none"
            style={{
              position: "absolute",
              bottom: 12,
              alignSelf: "center",
              backgroundColor: "#000a",
              paddingHorizontal: 12,
              paddingVertical: 6,
              borderRadius: 14,
            }}
          >
            <Text variant="footnote" style={{ color: "#fff" }}>
              Watching. Take control to touch the desktop.
            </Text>
          </View>
        ) : null}
      </View>
      <View style={{ position: "absolute", top: 0, left: 0, right: 0, padding: 12, paddingTop: embedded ? 12 : insets.top + 8 }} pointerEvents="box-none">
        {bar(
          <>
            {embedded ? null : round("close", "Close the live desktop", onClose)}
            <View style={{ flex: 1 }} />
            <Pressable
              onPress={() => {
                Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success).catch(() => {});
                onToggleControl(!controlled);
              }}
              accessibilityRole="button"
              accessibilityLabel={controlled ? "Give control back to the agent" : "Take control. This pauses an agent that is working."}
              style={{
                height: 38,
                borderRadius: 19,
                paddingHorizontal: 16,
                alignItems: "center",
                justifyContent: "center",
                backgroundColor: controlled ? c.warning : "#ffffff",
              }}
            >
              <Text weight="600" style={{ color: "#111" }}>
                {controlled ? "Give back" : "Take control"}
              </Text>
            </Pressable>
          </>,
        )}
        {controlled ? (
          <Text variant="caption" style={{ color: "#fff", marginTop: 6 }}>
            {controlNote ?? "Touch the desktop to control it. A working agent is paused."}
          </Text>
        ) : null}
      </View>
      {controlled ? (
        <View style={{ position: "absolute", bottom: 0, left: 0, right: 0, paddingBottom: 28, paddingHorizontal: 12, gap: 8 }}>
          {keys ? (
            <View style={{ flexDirection: "row", flexWrap: "wrap", gap: 6, justifyContent: "center" }}>
              {KEYS.map(([label, key]) => {
                const isMod = key === "ctrl" || key === "alt" || key === "cmd";
                const on = mods.includes(key);
                return (
                  <Pressable
                    key={key}
                    accessibilityRole="button"
                    accessibilityLabel={label}
                    accessibilityState={{ selected: on }}
                    onPress={() => {
                      Haptics.selectionAsync().catch(() => {});
                      if (isMod) setMods((m) => (on ? m.filter((x) => x !== key) : [...m, key]));
                      else {
                        send({ type: "key", key, modifiers: mods });
                        setMods([]);
                      }
                    }}
                    style={{
                      minWidth: 46,
                      height: 34,
                      paddingHorizontal: 10,
                      borderRadius: 8,
                      alignItems: "center",
                      justifyContent: "center",
                      backgroundColor: on ? "#fff" : "#ffffff30",
                    }}
                  >
                    <Text variant="footnote" weight="600" style={{ color: on ? "#111" : "#fff" }}>
                      {label}
                    </Text>
                  </Pressable>
                );
              })}
            </View>
          ) : null}
          {bar(
            <>
              <View style={{ flex: 1 }} />
              {round(
                "keyboard",
                "Keyboard",
                () => {
                  setTyping(true);
                  setTimeout(() => hiddenInput.current?.focus(), 30);
                },
                typing,
              )}
              {round("chevronUpDown", "Keys row", () => setKeys((k) => !k), keys)}
              {round("trackpad", trackpad ? "Touch mode" : "Trackpad mode", () => setTrackpad((t) => !t), trackpad)}
              {round("reload", "Reset zoom", () => {
                scale.value = 1;
                tx.value = 0;
                ty.value = 0;
                base.current = { scale: 1, tx: 0, ty: 0 };
              })}
              <View style={{ flex: 1 }} />
            </>,
          )}
        </View>
      ) : null}
      <TextInput
        ref={hiddenInput}
        value=""
        onChangeText={(t) => t && send({ type: "type", text: t })}
        onSubmitEditing={() => send({ type: "key", key: "return" })}
        onBlur={() => setTyping(false)}
        autoCapitalize="none"
        autoCorrect={false}
        blurOnSubmit={false}
        style={{ position: "absolute", width: 1, height: 1, opacity: 0 }}
        accessibilityLabel="Type on the desktop"
      />
    </View>
  );
}
