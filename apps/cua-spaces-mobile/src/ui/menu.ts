// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import * as Haptics from "expo-haptics";
import { ActionSheetIOS, Alert, Platform } from "react-native";

export interface MenuItem {
  label: string;
  destructive?: boolean;
  run(): void;
}

/** A long-press menu: the native action sheet on iOS, an alert list elsewhere. */
export function showMenu(title: string | undefined, items: MenuItem[]): void {
  Haptics.selectionAsync().catch(() => {});
  if (Platform.OS === "ios") {
    ActionSheetIOS.showActionSheetWithOptions(
      {
        title,
        options: [...items.map((i) => i.label), "Cancel"],
        cancelButtonIndex: items.length,
        destructiveButtonIndex: items.findIndex((i) => i.destructive) >= 0 ? items.findIndex((i) => i.destructive) : undefined,
      },
      (i) => {
        if (i < items.length) items[i]!.run();
      },
    );
  } else {
    Alert.alert(title ?? "", undefined, [
      ...items.map((i) => ({ text: i.label, style: i.destructive ? ("destructive" as const) : ("default" as const), onPress: i.run })),
      { text: "Cancel", style: "cancel" as const },
    ]);
  }
}
