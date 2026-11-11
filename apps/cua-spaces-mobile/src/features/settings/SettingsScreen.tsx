// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Constants from "expo-constants";
import { useEffect, useState, type ReactNode } from "react";
import { Alert, Linking, Pressable, ScrollView, Switch, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { ensureUnlocked, lockAvailable } from "../../auth/lock";
import { core } from "../../core";
import { pushSettingsView } from "../../core/phone";
import { useApp } from "../../state/app";
import { usePrefs } from "../../state/prefs";
import { askPermission, permissionState } from "../../notifications";
import { syncPush, usePushState, withdrawPush } from "../../notifications/push";
import { useTheme } from "../../theme/theme";
import { Text } from "../../ui/Text";

function Group({ title, footer, children }: { title?: string; footer?: string; children: ReactNode }) {
  const { c } = useTheme();
  return (
    <View style={{ marginTop: 22 }}>
      {title ? (
        <Text variant="footnote" color="muted" style={{ marginLeft: 32, marginBottom: 6, textTransform: "uppercase", letterSpacing: 0.4 }}>
          {title}
        </Text>
      ) : null}
      <View style={{ marginHorizontal: 16, borderRadius: 10, backgroundColor: c.card, overflow: "hidden" }}>{children}</View>
      {footer ? (
        <Text variant="footnote" color="muted" style={{ marginHorizontal: 32, marginTop: 6 }}>
          {footer}
        </Text>
      ) : null}
    </View>
  );
}

function Row({
  label,
  value,
  onPress,
  destructive,
  right,
  last,
}: {
  label: string;
  value?: string;
  onPress?(): void;
  destructive?: boolean;
  right?: ReactNode;
  last?: boolean;
}) {
  const { c } = useTheme();
  const body = (
    <View
      style={{
        minHeight: 46,
        paddingHorizontal: 16,
        flexDirection: "row",
        alignItems: "center",
        gap: 10,
        borderBottomWidth: last ? 0 : 0.5,
        borderBottomColor: c.border,
      }}
    >
      <Text style={{ flex: 1 }} color={destructive ? "destructive" : "foreground"}>
        {label}
      </Text>
      {value ? (
        <Text color="muted" numberOfLines={1} style={{ maxWidth: "55%" }}>
          {value}
        </Text>
      ) : null}
      {right}
    </View>
  );
  return onPress ? (
    <Pressable onPress={onPress} accessibilityRole="button" accessibilityLabel={label}>
      {body}
    </Pressable>
  ) : (
    body
  );
}

export function SettingsScreen({ onClose }: { onClose(): void }) {
  const { c } = useTheme();
  const insets = useSafeAreaInsets();
  const { account, signOut, backend, deviceId } = useApp();
  const prefs = usePrefs();
  const [perm, setPerm] = useState<"granted" | "denied" | "undetermined">("undetermined");
  useEffect(() => {
    permissionState().then(setPerm, () => {});
  }, []);
  const sample = backend?.capabilities.sample;
  const [canLock, setCanLock] = useState(true);
  useEffect(() => {
    lockAvailable().then(setCanLock, () => setCanLock(false));
  }, []);
  const push = usePushState();
  // The Notifications group: its rows, the one honest line about where push stands, and the action, are the core's.
  const notif = pushSettingsView({
    permission: perm,
    needsYou: prefs.notifyNeedsYou,
    done: prefs.notifyDone,
    registered: push.registered && push.configured,
    pushAvailable: !push.unavailable && push.configured,
  });
  const doAction = async (a: "allow" | "open-settings") => {
    if (a === "open-settings") await Linking.openSettings();
    else {
      const granted = await askPermission();
      setPerm(granted ? "granted" : "denied");
      // Allowed just now: the relay learns this phone's token at once.
      if (backend)
        await syncPush({ backend, deviceId: deviceId(), permission: granted ? "granted" : "denied", needsYou: prefs.notifyNeedsYou, done: prefs.notifyDone });
    }
  };
  return (
    <View style={{ flex: 1, backgroundColor: c.grouped }}>
      <View style={{ paddingTop: 16, paddingHorizontal: 16, flexDirection: "row", alignItems: "center" }}>
        <Text variant="title" style={{ flex: 1 }} accessibilityRole="header">
          Settings
        </Text>
        <Pressable onPress={onClose} accessibilityRole="button" accessibilityLabel="Done" hitSlop={10}>
          <Text weight="600" color="brand">
            Done
          </Text>
        </Pressable>
      </View>
      <ScrollView contentContainerStyle={{ paddingBottom: insets.bottom + 40 }}>
        <Group title="Account">
          {sample ? (
            <Row label="Sample data" value="Not your account" last />
          ) : (
            <>
              <Row label={account?.name ?? "Signed in"} value={account?.email ?? undefined} />
              <Row
                label="Sign out"
                destructive
                onPress={async () => {
                  if (!(await ensureUnlocked("Sign out of Cua Spaces", prefs.requireUnlock))) return;
                  onClose();
                  if (backend) void withdrawPush(backend);
                  signOut();
                }}
                last
              />
            </>
          )}
        </Group>
        <Group title={notif?.title ?? "Notifications"} footer={notif?.footer}>
          {(notif?.rows ?? []).map((r) => (
            <Row
              key={r.id}
              label={r.label}
              right={
                <Switch
                  value={r.on}
                  onValueChange={(v) => prefs.set(r.id === "needsYou" ? { notifyNeedsYou: v } : { notifyDone: v })}
                  accessibilityLabel={r.accessibilityLabel}
                  testID={`switch-${r.id}`}
                />
              }
            />
          ))}
          {notif ? (
            <Row
              label={notif.status.text}
              value={notif.status.actionLabel ?? undefined}
              onPress={notif.status.action ? () => void doAction(notif.status.action!) : undefined}
              last
            />
          ) : null}
        </Group>
        {sample || !backend?.removeAllPhones ? null : (
          <Group
            title="Lost a phone?"
            footer="Removes every phone from your account at once, this one too: their sessions, desktop views and alerts end. You sign in again on the ones you keep."
          >
            <Row
              label="Remove all phones"
              destructive
              last
              onPress={async () => {
                if (!(await ensureUnlocked("Remove all phones from your account", prefs.requireUnlock))) return;
                try {
                  const n = await backend.removeAllPhones!();
                  Alert.alert(n === 1 ? "1 phone removed" : `${n} phones removed`, "Sign in again on the ones you keep.");
                  onClose();
                  signOut();
                } catch (e) {
                  Alert.alert("Could not remove the phones", e instanceof Error ? e.message : String(e));
                }
              }}
            />
          </Group>
        )}
        {sample ? null : (
          <Group
            title="Privacy"
            footer={
              canLock
                ? "Asked when you open Cua Spaces and before you open the desktop of a computer."
                : "This phone has no passcode, so nothing can be asked. Set one in the iOS Settings app."
            }
          >
            <Row
              label="Require Face ID or passcode"
              right={
                <Switch
                  value={prefs.requireUnlock}
                  onValueChange={(v) => prefs.set({ requireUnlock: v })}
                  accessibilityLabel="Require Face ID or passcode"
                  testID="switch-lock"
                />
              }
              last
            />
          </Group>
        )}
        {sample && backend?.sample ? (
          <Group title="Sample data" footer="Makes this sample phone behave as if it had no connection, to see the offline states. Nothing else changes.">
            <Row
              label="Pretend this phone is offline"
              right={
                <Switch
                  value={prefs.sampleOffline}
                  onValueChange={(v) => prefs.set({ sampleOffline: v })}
                  accessibilityLabel="Pretend this phone is offline"
                  testID="switch-offline"
                />
              }
              last
            />
          </Group>
        ) : null}
        <Group title="About">
          <Row label="Version" value={Constants.expoConfig?.version ?? "0.1.0"} />
          <Row label="App core" value={core.available ? "Linked" : "Not in this build"} last />
        </Group>
      </ScrollView>
    </View>
  );
}
