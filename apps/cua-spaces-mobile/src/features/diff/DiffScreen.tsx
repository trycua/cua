// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Image } from "expo-image";
import { useCallback, useEffect, useMemo, useState } from "react";
import { ActivityIndicator, Pressable, ScrollView, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { diffChanges, diffFile, diffLabels, diffPreview, type ChangesView, type DiffLine, type FileDiffView, type PreviewView } from "../../core/phone";
import type { ChangeSet, FileDiffData, FilePreviewData } from "../../data/types";
import { useBackend } from "../../state/backend";
import { useTheme } from "../../theme/theme";
import { Icon } from "../../ui/Icon";
import { Text } from "../../ui/Text";

type Load<T> = { state: "loading" } | { state: "error"; message: string } | { state: "ready"; value: T };

/**
 * Reviewing what an agent changed in a Space: the files with their counts, one file's diff (unified, or with the words
 * that changed marked), and a preview of an image or a text file. The daemon sends the facts (`space_changes`,
 * `space_file_diff`, `space_file_preview`); every word and number here is the app core's (`diff.*`), the same ones the
 * push card uses, so the list, the card and the diff agree.
 */
export function DiffScreen({ spaceId, onBack, initialPath }: { spaceId: string; onBack(): void; initialPath?: string | null }) {
  const { c } = useTheme();
  const insets = useSafeAreaInsets();
  const backend = useBackend();
  const labels = diffLabels();
  const [changes, setChanges] = useState<Load<{ set: ChangeSet; view: ChangesView }>>({ state: "loading" });
  const [file, setFile] = useState<string | null>(initialPath ?? null);

  const load = useCallback(async () => {
    setChanges({ state: "loading" });
    try {
      if (!backend.changes) throw new Error(backend.capabilities.why.diff ?? labels?.unavailable ?? "Changes are not available.");
      const set = await backend.changes(spaceId);
      const view = diffChanges({
        repo: set.repo,
        branch: set.branch,
        base: set.base,
        headSha: set.headSha,
        files: set.files,
        truncated: set.truncated,
        verified: set.verified,
      });
      if (!view) throw new Error("The app core is not linked in this build.");
      setChanges({ state: "ready", value: { set, view } });
    } catch (e) {
      setChanges({ state: "error", message: e instanceof Error ? e.message : String(e) });
    }
  }, [backend, spaceId, labels?.unavailable]);

  useEffect(() => {
    void load();
  }, [load]);

  const header = (title: string, back: () => void, backLabel: string, sub?: string | null) => (
    <View style={{ paddingTop: insets.top + 6, paddingHorizontal: 12, paddingBottom: 8, flexDirection: "row", alignItems: "center", gap: 10 }}>
      <Pressable
        onPress={back}
        accessibilityRole="button"
        accessibilityLabel={backLabel}
        testID="diff-back"
        style={{ width: 38, height: 38, borderRadius: 19, backgroundColor: c.tile, alignItems: "center", justifyContent: "center" }}
      >
        <Icon name="back" size={18} color={c.foreground} weight="semibold" />
      </Pressable>
      <View style={{ flex: 1 }}>
        <Text variant="headline" numberOfLines={1} accessibilityRole="header">
          {title}
        </Text>
        {sub ? (
          <Text variant="footnote" color="muted" numberOfLines={1}>
            {sub}
          </Text>
        ) : null}
      </View>
    </View>
  );

  if (file && changes.state === "ready") {
    const f = changes.value.view.files.find((x) => x.path === file);
    return (
      <View style={{ flex: 1, backgroundColor: c.background }}>
        {header(
          f?.name ?? file,
          () => (initialPath ? onBack() : setFile(null)),
          labels?.back ?? "Back to changes",
          f ? `${f.dir}${f.statusLabel ? `  ${f.statusLabel}` : ""}`.trim() : null,
        )}
        <FileView
          spaceId={spaceId}
          path={file}
          meta={
            f
              ? {
                  binary: f.binary,
                  preview: f.preview,
                  mime: changes.value.set.files.find((x) => x.path === file)?.mime ?? null,
                  size: changes.value.set.files.find((x) => x.path === file)?.size ?? null,
                }
              : null
          }
        />
      </View>
    );
  }

  return (
    <View style={{ flex: 1, backgroundColor: c.background }}>
      {header(
        changes.state === "ready" ? changes.value.view.title : (labels?.title ?? "Changes"),
        onBack,
        "Back",
        changes.state === "ready" ? [changes.value.view.repo, changes.value.view.branchLine].filter(Boolean).join("  ") : null,
      )}
      {changes.state === "loading" ? (
        <View style={{ padding: 32, alignItems: "center", gap: 10 }} accessibilityLabel={labels?.loading ?? "Loading changes"}>
          <ActivityIndicator color={c.mutedForeground} />
          <Text variant="footnote" color="muted">
            {labels?.loading ?? ""}
          </Text>
        </View>
      ) : changes.state === "error" ? (
        <View style={{ padding: 28, alignItems: "center", gap: 10 }}>
          <Text variant="headline" style={{ textAlign: "center" }}>
            {labels?.unavailable ?? "Changes are not available."}
          </Text>
          <Text variant="subhead" color="muted" style={{ textAlign: "center" }}>
            {changes.message}
          </Text>
          <Pressable
            onPress={load}
            accessibilityRole="button"
            style={{ backgroundColor: c.primary, borderRadius: 10, paddingHorizontal: 16, paddingVertical: 9 }}
          >
            <Text weight="600" style={{ color: c.primaryForeground }}>
              {labels?.retry ?? "Try again"}
            </Text>
          </Pressable>
        </View>
      ) : (
        <ChangeList view={changes.value.view} onOpen={setFile} bottom={insets.bottom} />
      )}
    </View>
  );
}

function Squares({ bar }: { bar: ("add" | "del" | "none")[] }) {
  const { c } = useTheme();
  return (
    <View style={{ flexDirection: "row", gap: 2 }} accessible={false}>
      {bar.map((s, i) => (
        <View key={i} style={{ width: 8, height: 8, borderRadius: 1.5, backgroundColor: s === "add" ? c.diffAdd : s === "del" ? c.diffDel : c.border }} />
      ))}
    </View>
  );
}

function ChangeList({ view, onOpen, bottom }: { view: ChangesView; onOpen(path: string): void; bottom: number }) {
  const { c, mono } = useTheme();
  if (view.empty)
    return (
      <View style={{ padding: 32, alignItems: "center" }}>
        <Text color="muted">{view.empty}</Text>
      </View>
    );
  return (
    <ScrollView contentContainerStyle={{ paddingHorizontal: 16, paddingBottom: bottom + 24 }}>
      <View
        style={{ flexDirection: "row", gap: 10, alignItems: "baseline", paddingVertical: 8 }}
        accessible
        accessibilityLabel={`${view.filesTotal}, ${view.additions} added, ${view.deletions} removed`}
      >
        <Text variant="subhead" weight="600">
          {view.filesTotal}
        </Text>
        {view.additions ? (
          <Text variant="subhead" weight="600" style={{ color: c.diffAdd, fontFamily: mono }}>
            {view.additions}
          </Text>
        ) : null}
        {view.deletions ? (
          <Text variant="subhead" weight="600" style={{ color: c.diffDel, fontFamily: mono }}>
            {view.deletions}
          </Text>
        ) : null}
      </View>
      <View style={{ borderRadius: 12, backgroundColor: c.card, borderWidth: 0.5, borderColor: c.border, overflow: "hidden" }}>
        {view.files.map((f, i) => (
          <Pressable
            key={f.path}
            onPress={() => onOpen(f.path)}
            accessibilityRole="button"
            accessibilityLabel={`${f.dir}${f.name}, ${f.statusLabel}, ${f.additions} ${f.deletions}`.replace(/\s+/g, " ").trim()}
            testID={`diff-file-${f.name}`}
            style={({ pressed }) => ({
              paddingHorizontal: 12,
              paddingVertical: 10,
              flexDirection: "row",
              alignItems: "center",
              gap: 10,
              borderTopWidth: i ? 0.5 : 0,
              borderTopColor: c.border,
              backgroundColor: pressed ? c.accent : f.status === "added" ? c.diffAddSurface : f.status === "deleted" ? c.diffDelSurface : "transparent",
            })}
          >
            <View style={{ flex: 1 }}>
              <Text variant="subhead" numberOfLines={1}>
                <Text variant="subhead" color="muted">
                  {f.dir}
                </Text>
                <Text variant="subhead" weight="600">
                  {f.name}
                </Text>
              </Text>
              <Text variant="caption" color="muted">
                {[f.statusLabel, f.note].filter(Boolean).join(", ")}
              </Text>
            </View>
            {f.additions ? (
              <Text variant="caption" weight="600" style={{ color: c.diffAdd, fontFamily: mono }}>
                {f.additions}
              </Text>
            ) : null}
            {f.deletions ? (
              <Text variant="caption" weight="600" style={{ color: c.diffDel, fontFamily: mono }}>
                {f.deletions}
              </Text>
            ) : null}
            <Squares bar={f.bar} />
            <Icon name="forward" size={12} color={c.mutedForeground} />
          </Pressable>
        ))}
      </View>
      {view.truncatedNote ? (
        <Text variant="caption" color="muted" style={{ marginTop: 6 }}>
          {view.truncatedNote}
        </Text>
      ) : null}
      {view.verifiedNote ? (
        <Text variant="caption" color="muted" style={{ marginTop: 6 }} testID="verified-note">
          {view.verifiedNote}
        </Text>
      ) : null}
    </ScrollView>
  );
}

type Tab = "diff" | "preview";

function FileView({
  spaceId,
  path,
  meta,
}: {
  spaceId: string;
  path: string;
  meta: { binary: boolean; preview: "image" | "text" | "none"; mime: string | null; size: number | null } | null;
}) {
  const { c } = useTheme();
  const insets = useSafeAreaInsets();
  const backend = useBackend();
  const labels = diffLabels();
  const canPreview = meta?.preview !== "none" && !!backend.filePreview;
  // An image has no text diff: it opens on its preview.
  const [tab, setTab] = useState<Tab>(meta?.binary && canPreview ? "preview" : "diff");
  const [words, setWords] = useState(false);
  const [diff, setDiff] = useState<Load<FileDiffView>>({ state: "loading" });
  const [preview, setPreview] = useState<Load<{ data: FilePreviewData; view: PreviewView }>>({ state: "loading" });

  useEffect(() => {
    let alive = true;
    (async () => {
      try {
        if (!backend.fileDiff) throw new Error(labels?.unavailable ?? "Not available.");
        const d: FileDiffData = await backend.fileDiff(spaceId, path);
        const v = diffFile({ path, patch: d.patch, truncated: d.truncated, binary: d.binary || !!meta?.binary });
        if (!v) throw new Error("The app core is not linked in this build.");
        if (alive) setDiff({ state: "ready", value: v });
      } catch (e) {
        if (alive) setDiff({ state: "error", message: e instanceof Error ? e.message : String(e) });
      }
    })();
    return () => {
      alive = false;
    };
  }, [backend, spaceId, path, meta?.binary, labels?.unavailable]);

  useEffect(() => {
    if (tab !== "preview" || !backend.filePreview) return;
    let alive = true;
    (async () => {
      try {
        const data = await backend.filePreview!(spaceId, path, { rev: "head" });
        const view = diffPreview({
          path,
          mime: data.mime ?? meta?.mime ?? null,
          size: data.size ?? meta?.size ?? null,
          binary: meta?.binary,
          truncated: data.truncated,
        });
        if (!view) throw new Error("The app core is not linked in this build.");
        if (alive) setPreview({ state: "ready", value: { data, view } });
      } catch (e) {
        if (alive) setPreview({ state: "error", message: e instanceof Error ? e.message : String(e) });
      }
    })();
    return () => {
      alive = false;
    };
  }, [backend, spaceId, path, tab, meta?.binary, meta?.mime, meta?.size]);

  const tabs: Tab[] = canPreview ? ["diff", "preview"] : ["diff"];
  return (
    <View style={{ flex: 1 }}>
      <View style={{ flexDirection: "row", gap: 8, paddingHorizontal: 16, paddingBottom: 8 }}>
        {tabs.length > 1
          ? tabs.map((t) => (
              <Chip
                key={t}
                label={t === "diff" ? (labels?.diffTab ?? "Diff") : (labels?.previewTab ?? "Preview")}
                on={tab === t}
                onPress={() => setTab(t)}
                testID={`tab-${t}`}
              />
            ))
          : null}
        {tab === "diff" && diff.state === "ready" && diff.value.hasWords ? (
          <>
            <View style={{ flex: 1 }} />
            <Chip label={diff.value.unifiedLabel} on={!words} onPress={() => setWords(false)} testID="mode-unified" />
            <Chip label={diff.value.wordsLabel} on={words} onPress={() => setWords(true)} testID="mode-words" />
          </>
        ) : null}
      </View>
      {tab === "diff" ? <DiffBody load={diff} words={words} bottom={insets.bottom} /> : <PreviewBody load={preview} bottom={insets.bottom} />}
    </View>
  );
}

function Chip({ label, on, onPress, testID }: { label: string; on: boolean; onPress(): void; testID?: string }) {
  const { c } = useTheme();
  return (
    <Pressable
      onPress={onPress}
      accessibilityRole="button"
      accessibilityState={{ selected: on }}
      testID={testID}
      style={{ height: 32, borderRadius: 16, paddingHorizontal: 14, alignItems: "center", justifyContent: "center", backgroundColor: on ? c.primary : c.tile }}
    >
      <Text variant="footnote" weight="600" style={{ color: on ? c.primaryForeground : c.foreground }}>
        {label}
      </Text>
    </Pressable>
  );
}

function Busy() {
  const { c } = useTheme();
  const labels = diffLabels();
  return (
    <View style={{ padding: 28, alignItems: "center", gap: 8 }} accessibilityLabel={labels?.loadingFile ?? "Loading"}>
      <ActivityIndicator color={c.mutedForeground} />
    </View>
  );
}

function Failed({ message }: { message: string }) {
  return (
    <View style={{ padding: 28 }}>
      <Text color="muted" style={{ textAlign: "center" }}>
        {message}
      </Text>
    </View>
  );
}

function DiffBody({ load, words, bottom }: { load: Load<FileDiffView>; words: boolean; bottom: number }) {
  const { c, mono } = useTheme();
  if (load.state === "loading") return <Busy />;
  if (load.state === "error") return <Failed message={load.message} />;
  const v = load.value;
  return (
    <ScrollView contentContainerStyle={{ paddingBottom: bottom + 24 }}>
      <View
        style={{ flexDirection: "row", gap: 10, paddingHorizontal: 16, paddingBottom: 6 }}
        accessible
        accessibilityLabel={`${v.additions} added, ${v.deletions} removed`}
      >
        {v.additions ? (
          <Text variant="footnote" weight="600" style={{ color: c.diffAdd, fontFamily: mono }}>
            {v.additions}
          </Text>
        ) : null}
        {v.deletions ? (
          <Text variant="footnote" weight="600" style={{ color: c.diffDel, fontFamily: mono }}>
            {v.deletions}
          </Text>
        ) : null}
      </View>
      {v.notes.map((n) => (
        <Text key={n} variant="footnote" color="muted" style={{ paddingHorizontal: 16, paddingVertical: 6 }}>
          {n}
        </Text>
      ))}
      {v.hunks.map((h, i) => (
        <View key={i} style={{ marginBottom: 10 }}>
          <View style={{ backgroundColor: c.tile, paddingHorizontal: 16, paddingVertical: 4 }}>
            <Text variant="caption" color="muted" style={{ fontFamily: mono }} numberOfLines={1}>
              {h.header}
            </Text>
          </View>
          {h.lines.map((l, j) => (
            <Line key={j} line={l} words={words} />
          ))}
        </View>
      ))}
    </ScrollView>
  );
}

function Line({ line, words }: { line: DiffLine; words: boolean }) {
  const { c, mono } = useTheme();
  const bg = line.kind === "add" ? c.diffAddSurface : line.kind === "del" ? c.diffDelSurface : "transparent";
  const mark = line.kind === "add" ? "+" : line.kind === "del" ? "-" : " ";
  const tone = line.kind === "add" ? c.diffAdd : line.kind === "del" ? c.diffDel : c.mutedForeground;
  const segs = useMemo(() => (words && line.segments.length ? line.segments : null), [words, line.segments]);
  const strong = line.kind === "add" ? c.diffAdd + "40" : c.diffDel + "40";
  return (
    <View
      style={{ flexDirection: "row", backgroundColor: bg, paddingRight: 12 }}
      accessible
      accessibilityLabel={`${line.kind === "add" ? "Added" : line.kind === "del" ? "Removed" : "Unchanged"}, line ${line.newNo ?? line.oldNo}: ${line.text}`}
    >
      <Text variant="caption" color="muted" style={{ width: 34, textAlign: "right", fontFamily: mono, paddingRight: 4 }}>
        {line.oldNo ?? ""}
      </Text>
      <Text variant="caption" color="muted" style={{ width: 34, textAlign: "right", fontFamily: mono, paddingRight: 6 }}>
        {line.newNo ?? ""}
      </Text>
      <Text variant="caption" weight="700" style={{ width: 12, color: tone, fontFamily: mono }}>
        {mark}
      </Text>
      <Text variant="caption" style={{ flex: 1, fontFamily: mono }}>
        {segs
          ? segs.map((s, k) =>
              s.changed ? (
                <Text key={k} variant="caption" weight="700" testID="changed-segment" style={{ fontFamily: mono, backgroundColor: strong }}>
                  {s.text}
                </Text>
              ) : (
                s.text
              ),
            )
          : line.text}
      </Text>
    </View>
  );
}

function PreviewBody({ load, bottom }: { load: Load<{ data: FilePreviewData; view: PreviewView }>; bottom: number }) {
  const { c, mono } = useTheme();
  const labels = diffLabels();
  if (load.state === "loading") return <Busy />;
  if (load.state === "error") return <Failed message={load.message} />;
  const { data, view } = load.value;
  if (view.kind === "none") return <Failed message={view.note ?? ""} />;
  return (
    <ScrollView contentContainerStyle={{ padding: 16, paddingBottom: bottom + 24, gap: 10 }}>
      {view.sizeLabel ? (
        <Text variant="caption" color="muted">
          {view.name}, {view.sizeLabel}
        </Text>
      ) : null}
      {view.kind === "image" && data.encoding === "base64" ? (
        <Image
          source={{ uri: `data:${data.mime ?? "image/png"};base64,${data.content}` }}
          contentFit="contain"
          accessibilityLabel={`${labels?.imageAlt ?? "Preview"}: ${view.name}`}
          style={{ width: "100%", aspectRatio: 1.6, borderRadius: 10, backgroundColor: c.tile }}
          testID="preview-image"
        />
      ) : (
        <View style={{ backgroundColor: c.card, borderRadius: 10, padding: 12, borderWidth: 0.5, borderColor: c.border }}>
          <Text variant="mono" selectable style={{ fontFamily: mono }} testID="preview-text">
            {data.content}
          </Text>
        </View>
      )}
      {view.note ? (
        <Text variant="caption" color="muted">
          {view.note}
        </Text>
      ) : null}
    </ScrollView>
  );
}
