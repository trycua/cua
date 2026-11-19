// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import * as Haptics from "expo-haptics";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ActivityIndicator, KeyboardAvoidingView, Platform, Pressable, ScrollView, TextInput, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import {
  promptSuggestions,
  startTaskPlan,
  startTaskView,
  type PromptSuggestion,
  type StartTaskInput,
  type StartTaskView,
  type TaskChoice,
  type WhereChoice,
} from "../../core/phone";
import type { TaskOptions } from "../../data/types";
import { useBackend, useSpaceList } from "../../state/backend";
import { randomId } from "../../store/outbox";
import { useOutbox } from "../../state/stores";
import { useTheme } from "../../theme/theme";
import { Icon } from "../../ui/Icon";
import { Text } from "../../ui/Text";

type Options = { state: "loading" } | { state: "error"; message: string } | { state: "ready"; options: TaskOptions };

/**
 * Start a task from the phone: one box, an agent, where it runs (Let Cua decide, a machine, or a Space that exists),
 * and optionally a repository. It is the web Home's flow: the job becomes the first message `@agent job`, and the
 * machine's daemon makes the Space (or finds the agent's home). Every word is the app core's (`startTask.view`); the
 * job is written down in the outbox first, so a lost connection keeps it and it starts exactly once.
 */
export function StartTaskSheet({
  onClose,
  onStarted,
  newId = randomId,
}: {
  onClose(): void;
  onStarted(spaceId: string): void;
  /** The id of a task (a test gives one it controls). */ newId?: () => string;
}) {
  const { c } = useTheme();
  const insets = useSafeAreaInsets();
  const backend = useBackend();
  const outbox = useOutbox();
  const { list } = useSpaceList();
  const [options, setOptions] = useState<Options>({ state: "loading" });
  const [choice, setChoice] = useState<TaskChoice>({});
  const [prompt, setPrompt] = useState("");
  const [open, setOpen] = useState<"agent" | "where" | "repo" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [starting, setStarting] = useState(false);
  /** The machine is making the Space (it can take minutes): the sheet follows it, or says why it did not start. */
  const [following, setFollowing] = useState<{ state: "creating" } | { state: "failed"; error: string } | null>(null);
  const clientId = useRef(newId());
  const input = useRef<React.ComponentRef<typeof TextInput>>(null);

  const loadOptions = useCallback(async () => {
    setOptions({ state: "loading" });
    try {
      if (!backend.taskOptions) throw new Error(backend.capabilities.why.startTask ?? "This account cannot start a task from the phone yet.");
      setOptions({ state: "ready", options: await backend.taskOptions() });
    } catch (e) {
      setOptions({ state: "error", message: e instanceof Error ? e.message : String(e) });
    }
  }, [backend]);
  useEffect(() => {
    void loadOptions();
  }, [loadOptions]);

  // Existing Spaces the job could go to: an online one with an agent already in it.
  const spaces = useMemo(
    () =>
      (list?.spaces ?? [])
        .filter((s) => s.machineOnline && s.role !== "viewer")
        .map((s) => ({
          id: s.id,
          name: s.name,
          machineId: s.machineId,
          online: s.machineOnline,
          agents: s.members.filter((m) => m.kind === "agent").map((m) => ({ handle: m.id, harness: m.harness ?? "", label: m.name })),
        }))
        .filter((s) => s.agents.length > 0),
    [list],
  );
  const taskInput: StartTaskInput | null = useMemo(
    () =>
      options.state === "ready"
        ? {
            machines: options.options.machines.map((m) => ({
              id: m.id,
              name: m.name,
              online: m.online,
              canCreate: m.canCreate,
              why: m.why,
              os: m.os,
              readyAgents: m.readyAgents,
            })),
            agents: options.options.agents,
            spaces,
            choice,
          }
        : null,
    [options, spaces, choice],
  );
  const view: StartTaskView | null = taskInput ? startTaskView(taskInput) : null;
  const suggestions: PromptSuggestion[] = useMemo(() => promptSuggestions(), []);

  /** What the machine answered: open the Space, follow it while it is made, or say why it did not start. */
  const follow = (t: { state: "ready" | "creating" | "failed"; space: string; error: string | null }) => {
    if (t.state === "ready") onStarted(t.space);
    else if (t.state === "failed") {
      setFollowing({ state: "failed", error: t.error ?? "The computer could not make it." });
      setStarting(false);
    } else {
      setFollowing({ state: "creating" });
      const id = clientId.current;
      const poll = async () => {
        try {
          const r = await backend.taskStatus!(id);
          if (r.state === "creating") return void (alive.current && setTimeout(poll, 1_500));
          if (alive.current) follow(r);
        } catch (e) {
          // Cannot ask now (offline): the task keeps going; ask again shortly.
          if (alive.current) setTimeout(poll, 3_000);
        }
      };
      void poll();
    }
  };
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const start = async () => {
    if (!taskInput || !view || starting) return;
    const plan = startTaskPlan(taskInput, prompt, clientId.current);
    if (!plan) {
      setError("The app core is not linked in this build, so a task cannot be started from here.");
      return;
    }
    if (!plan.ok || !plan.request) {
      setError(plan.error);
      return;
    }
    setError(null);
    setStarting(true);
    Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Light).catch(() => {});
    try {
      if (outbox) {
        // Written down first: a lost connection keeps the job, and the same id never starts it twice.
        const { done } = await outbox.send({ space: `task:${plan.request.client_id}`, kind: "task", text: prompt.trim(), extra: { request: plan.request } });
        const settled = await Promise.race([done, new Promise<null>((r) => setTimeout(() => r(null), 3_000))]);
        if (settled?.task) follow(settled.task);
        else onClose();
      } else {
        follow(await backend.startTask!(plan.request));
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setStarting(false);
    }
  };

  const card = { backgroundColor: c.card, borderRadius: 14, borderWidth: 0.5, borderColor: c.border } as const;
  const chip = (label: string, on: boolean, onPress: () => void, testID: string, icon?: "branch") => (
    <Pressable
      onPress={onPress}
      accessibilityRole="button"
      accessibilityState={{ expanded: on }}
      accessibilityLabel={label}
      testID={testID}
      style={{
        height: 34,
        borderRadius: 17,
        paddingHorizontal: 12,
        flexDirection: "row",
        gap: 6,
        alignItems: "center",
        backgroundColor: on ? c.accent : c.tile,
        maxWidth: "100%",
      }}
    >
      {icon ? <Icon name={icon} size={13} color={c.mutedForeground} /> : null}
      <Text variant="footnote" weight="600" numberOfLines={1} style={{ flexShrink: 1 }}>
        {label}
      </Text>
      <Icon name="chevronUpDown" size={11} color={c.mutedForeground} />
    </Pressable>
  );
  const row = (key: string, label: string, detail: string | null, selected: boolean, enabled: boolean, onPress: () => void, last: boolean) => (
    <Pressable
      key={key}
      disabled={!enabled}
      onPress={onPress}
      accessibilityRole="button"
      accessibilityState={{ selected, disabled: !enabled }}
      accessibilityLabel={`${label}${detail ? `, ${detail}` : ""}`}
      style={{
        minHeight: 48,
        paddingHorizontal: 14,
        paddingVertical: 8,
        flexDirection: "row",
        alignItems: "center",
        gap: 10,
        borderBottomWidth: last ? 0 : 0.5,
        borderBottomColor: c.border,
        opacity: enabled ? 1 : 0.5,
      }}
    >
      <View style={{ flex: 1 }}>
        <Text weight="500">{label}</Text>
        {detail ? (
          <Text variant="footnote" color="muted">
            {detail}
          </Text>
        ) : null}
      </View>
      {selected ? <Icon name="check" size={15} color={c.brandStrong} weight="bold" /> : null}
    </Pressable>
  );
  const selectedAgent = view?.agents.find((a) => a.selected);
  const selectedWhere = view?.wheres.find((w) => w.selected);
  const canStart = !!view && !view.blocker && prompt.trim().length > 0 && !starting;

  return (
    <KeyboardAvoidingView behavior={Platform.OS === "ios" ? "padding" : undefined} style={{ flex: 1, backgroundColor: c.grouped }}>
      <View style={{ paddingTop: 16, paddingHorizontal: 16, flexDirection: "row", alignItems: "center" }}>
        <Pressable
          onPress={onClose}
          accessibilityRole="button"
          accessibilityLabel={following?.state === "creating" ? (view?.starting.leave ?? "Close") : (view?.cancelLabel ?? "Cancel")}
          hitSlop={10}
          testID="task-cancel"
        >
          <Text color="brand" weight="500">
            {following?.state === "creating" ? (view?.starting.leave ?? "Close") : (view?.cancelLabel ?? "Cancel")}
          </Text>
        </Pressable>
        <View style={{ flex: 1 }} />
        {backend.capabilities.sample ? (
          <Text variant="caption" color="warning">
            Sample data
          </Text>
        ) : null}
      </View>
      <ScrollView keyboardShouldPersistTaps="handled" contentContainerStyle={{ padding: 16, paddingBottom: 24, gap: 14 }}>
        {following && view ? (
          <View style={{ padding: 28, alignItems: "center", gap: 12 }} accessibilityLiveRegion="polite" testID="task-following">
            {following.state === "creating" ? <ActivityIndicator color={c.mutedForeground} /> : null}
            <Text variant="title" style={{ textAlign: "center" }}>
              {following.state === "creating" ? view.starting.title : view.starting.failedTitle}
            </Text>
            <Text variant="subhead" color="muted" style={{ textAlign: "center" }}>
              {following.state === "creating" ? view.starting.detail : following.error}
            </Text>
            {following.state === "failed" ? (
              <Pressable
                onPress={() => {
                  clientId.current = newId();
                  setFollowing(null);
                }}
                accessibilityRole="button"
                testID="task-retry"
                style={{ backgroundColor: c.primary, borderRadius: 10, paddingHorizontal: 16, paddingVertical: 9 }}
              >
                <Text weight="600" style={{ color: c.primaryForeground }}>
                  {view.starting.retry}
                </Text>
              </Pressable>
            ) : null}
          </View>
        ) : options.state === "loading" ? (
          <View style={{ padding: 40, alignItems: "center" }} accessibilityLabel="Loading">
            <ActivityIndicator color={c.mutedForeground} />
          </View>
        ) : options.state === "error" || !view ? (
          <View style={{ padding: 24, alignItems: "center", gap: 10 }}>
            <Text variant="headline" style={{ textAlign: "center" }}>
              {options.state === "error" ? options.message : "Could not load your computers."}
            </Text>
            <Pressable
              onPress={loadOptions}
              accessibilityRole="button"
              testID="task-options-retry"
              style={{ backgroundColor: c.primary, borderRadius: 10, paddingHorizontal: 16, paddingVertical: 9 }}
            >
              <Text weight="600" style={{ color: c.primaryForeground }}>
                Try again
              </Text>
            </Pressable>
          </View>
        ) : (
          <>
            <Text variant="title" accessibilityRole="header">
              {view.title}
            </Text>
            <View style={{ ...card, padding: 12, minHeight: 120 }}>
              <TextInput
                ref={input}
                value={prompt}
                onChangeText={setPrompt}
                placeholder={view.placeholder}
                placeholderTextColor={c.mutedForeground}
                multiline
                autoFocus
                style={{ minHeight: 90, color: c.foreground, fontSize: 17, paddingTop: 4 }}
                accessibilityLabel={view.placeholder}
                testID="task-input"
              />
              <View style={{ flexDirection: "row", flexWrap: "wrap", gap: 8, marginTop: 6 }}>
                {chip(selectedAgent?.label ?? view.agentTitle, open === "agent", () => setOpen(open === "agent" ? null : "agent"), "task-agent")}
                {chip(selectedWhere?.label ?? view.whereTitle, open === "where", () => setOpen(open === "where" ? null : "where"), "task-where")}
                {chip(view.repo.label, open === "repo", () => setOpen(open === "repo" ? null : "repo"), "task-repo", "branch")}
              </View>
            </View>
            {open === "agent" ? (
              <View style={{ ...card, overflow: "hidden" }} testID="task-agent-list">
                {view.agents.map((a, i) =>
                  row(
                    a.harness,
                    a.label,
                    a.detail,
                    a.selected,
                    a.enabled,
                    () => {
                      setChoice((x) => ({ ...x, agent: a.harness }));
                      setOpen(null);
                    },
                    i === view.agents.length - 1,
                  ),
                )}
              </View>
            ) : null}
            {open === "where" ? (
              <View style={{ ...card, overflow: "hidden" }} testID="task-where-list">
                {view.wheres.map((w, i) =>
                  row(
                    w.id,
                    w.label,
                    w.detail,
                    w.selected,
                    w.enabled,
                    () => {
                      const next: WhereChoice =
                        w.kind === "auto"
                          ? { kind: "auto" }
                          : w.kind === "machine"
                            ? { kind: "machine", machine: w.id.slice("machine:".length) }
                            : { kind: "space", space: w.id.slice("space:".length) };
                      setChoice((x) => ({ ...x, where: next }));
                      setOpen(null);
                    },
                    i === view.wheres.length - 1,
                  ),
                )}
              </View>
            ) : null}
            {open === "repo" ? (
              <View style={{ ...card, padding: 12, gap: 6 }} testID="task-repo-box">
                <TextInput
                  value={choice.repo ?? ""}
                  onChangeText={(t) => setChoice((x) => ({ ...x, repo: t }))}
                  placeholder={view.repo.placeholder}
                  placeholderTextColor={c.mutedForeground}
                  autoCapitalize="none"
                  autoCorrect={false}
                  keyboardType="url"
                  style={{ color: c.foreground, fontSize: 16, minHeight: 36 }}
                  accessibilityLabel={view.repo.placeholder}
                  testID="task-repo-input"
                />
                <Text variant="caption" color={view.repo.problem ? "destructive" : "muted"}>
                  {view.repo.problem ?? view.repo.hint}
                </Text>
                {view.repo.set ? (
                  <Pressable
                    onPress={() => {
                      setChoice((x) => ({ ...x, repo: null }));
                      setOpen(null);
                    }}
                    accessibilityRole="button"
                    hitSlop={8}
                  >
                    <Text variant="footnote" weight="600" color="brand">
                      {view.repo.clearLabel}
                    </Text>
                  </Pressable>
                ) : null}
              </View>
            ) : null}
            <Pressable
              onPress={start}
              disabled={!canStart}
              accessibilityRole="button"
              accessibilityLabel={view.startLabel}
              accessibilityState={{ disabled: !canStart }}
              testID="task-start"
              style={{ height: 48, borderRadius: 14, alignItems: "center", justifyContent: "center", backgroundColor: canStart ? c.primary : c.muted }}
            >
              {starting ? (
                <ActivityIndicator color={c.primaryForeground} />
              ) : (
                <Text weight="600" style={{ color: canStart ? c.primaryForeground : c.mutedForeground }}>
                  {view.startLabel}
                </Text>
              )}
            </Pressable>
            {!prompt.trim() && suggestions.length ? (
              <View style={{ gap: 8 }} testID="task-suggestions">
                {suggestions.map((s) => (
                  <Pressable
                    key={s.id}
                    onPress={() => {
                      setPrompt(s.prompt);
                      input.current?.focus();
                    }}
                    accessibilityRole="button"
                    accessibilityLabel={`${s.title}. ${s.detail}`}
                    style={({ pressed }) => ({ ...card, padding: 14, gap: 3, backgroundColor: pressed ? c.accent : c.card })}
                  >
                    <Text weight="600">{s.title}</Text>
                    <Text variant="footnote" color="muted">
                      {s.detail}
                    </Text>
                  </Pressable>
                ))}
              </View>
            ) : null}
            {view.blocker || error ? (
              <Text variant="footnote" color={error ? "destructive" : "muted"} accessibilityLiveRegion="polite" testID="task-note">
                {error ?? view.blocker}
              </Text>
            ) : null}
          </>
        )}
      </ScrollView>
    </KeyboardAvoidingView>
  );
}
