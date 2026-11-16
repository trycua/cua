// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Ionicons } from "@expo/vector-icons";
import { SymbolView, type SymbolViewProps } from "expo-symbols";
import { Platform } from "react-native";

/** SF Symbols on iOS; the same glyphs by Ionicons name elsewhere. */
const MAP = {
  back: ["chevron.left", "chevron-back"],
  forward: ["chevron.right", "chevron-forward"],
  down: ["chevron.down", "chevron-down"],
  plus: ["plus", "add"],
  close: ["xmark", "close"],
  send: ["arrow.up", "arrow-up"],
  stop: ["stop.fill", "stop"],
  search: ["magnifyingglass", "search"],
  more: ["ellipsis", "ellipsis-horizontal"],
  moon: ["moon.zzz", "moon"],
  share: ["square.and.arrow.up", "share-outline"],
  screen: ["display", "desktop-outline"],
  mac: ["laptopcomputer", "laptop-outline"],
  pc: ["desktopcomputer", "desktop-outline"],
  cloud: ["cloud", "cloud-outline"],
  iphone: ["iphone", "phone-portrait-outline"],
  ipad: ["ipad", "tablet-portrait-outline"],
  linux: ["terminal", "terminal-outline"],
  people: ["person.2", "people-outline"],
  bell: ["bell", "notifications-outline"],
  check: ["checkmark", "checkmark"],
  bolt: ["bolt.fill", "flash"],
  expand: ["arrow.up.left.and.arrow.down.right", "expand-outline"],
  collapse: ["arrow.down.right.and.arrow.up.left", "contract-outline"],
  keyboard: ["keyboard", "keypad-outline"],
  hand: ["hand.point.up.left", "hand-left-outline"],
  trackpad: ["rectangle.and.hand.point.up.left", "radio-button-on-outline"],
  gear: ["gearshape", "settings-outline"],
  reload: ["arrow.clockwise", "refresh"],
  doc: ["doc.text", "document-text-outline"],
  chevronUpDown: ["chevron.up.chevron.down", "swap-vertical"],
  mic: ["mic", "mic-outline"],
  at: ["at", "at"],
  tool: ["wrench.and.screwdriver", "construct-outline"],
  plusLine: ["plus.square", "add-circle-outline"],
  minusLine: ["minus.square", "remove-circle-outline"],
  lock: ["lock", "lock-closed-outline"],
  compose: ["square.and.pencil", "create-outline"],
  diff: ["plus.forwardslash.minus", "git-compare-outline"],
  photo: ["photo", "image-outline"],
  micFill: ["mic.fill", "mic"],
  branch: ["arrow.triangle.branch", "git-branch-outline"],
} as const;

export type IconName = keyof typeof MAP;

export function Icon({ name, size = 20, color, weight = "medium" }: { name: IconName; size?: number; color: string; weight?: SymbolViewProps["weight"] }) {
  const [sf, ion] = MAP[name];
  if (Platform.OS === "ios")
    return (
      <SymbolView
        name={sf as SymbolViewProps["name"]}
        size={size}
        tintColor={color}
        weight={weight}
        resizeMode="scaleAspectFit"
        style={{ width: size, height: size }}
      />
    );
  return <Ionicons name={ion as keyof typeof Ionicons.glyphMap} size={size} color={color} />;
}
