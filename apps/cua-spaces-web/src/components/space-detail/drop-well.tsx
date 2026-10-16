// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { ArrowDownCircleIcon } from "lucide-react";
import { useState, type DragEvent } from "react";

import { useSpaceFiles } from "@/bridge";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

/**
 * A Space's Teleport section, as the SwiftUI detail's `TeleportDropZone`: a
 * tall well with a dashed outline, its glyph and caption, and "Send file…"
 * and "Teleport an app…" below. While files are over it the outline turns
 * solid and the well fills. Dropped files go into the Space; an app bundle
 * opens Teleport at that app. The line under the buttons says what the last
 * drop did.
 */
export function DropWell({
  title,
  space,
  copy,
  onTeleport,
}: {
  title: string;
  space: { id: string; name: string };
  copy: { caption: string; sendFile: string; teleportApp: string };
  onTeleport: () => void;
}) {
  const files = useSpaceFiles(space);
  const [over, setOver] = useState(false);
  const carriesFiles = (e: DragEvent) => Array.from(e.dataTransfer?.types ?? []).includes("Files");
  const onDragOver = (e: DragEvent) => {
    if (!carriesFiles(e)) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = "copy";
    setOver(true);
  };
  const onDrop = (e: DragEvent) => {
    if (!carriesFiles(e)) return;
    e.preventDefault();
    setOver(false);
    const names = Array.from(e.dataTransfer.files).map((f) => f.name);
    if (names.length) void files.drop(names);
  };
  const status = files.status;
  return (
    <section data-section="teleport" className="mb-7">
      <h2 className="mb-2 px-1 text-xs font-semibold text-muted-foreground">{title}</h2>
      <div className="rounded-xl border bg-card p-2 shadow-xs">
        <div
          data-drop-well={space.id}
          data-drop-over={over ? "true" : undefined}
          onDragEnter={onDragOver}
          onDragOver={onDragOver}
          onDragLeave={() => setOver(false)}
          onDrop={onDrop}
          className={cn(
            "flex h-36 flex-col items-center justify-center gap-2 rounded-lg border-2 border-dashed text-muted-foreground transition-colors",
            over ? "border-solid border-brand bg-brand/10 text-foreground" : "border-border",
          )}
        >
          <ArrowDownCircleIcon className="size-7" strokeWidth={1.5} />
          <p className="text-[13px]">{copy.caption}</p>
        </div>
        <div className="flex items-center justify-center gap-2 pt-2">
          <Button variant="outline" size="sm" onClick={() => void files.choose()}>
            {copy.sendFile}
          </Button>
          <Button variant="outline" size="sm" onClick={onTeleport}>
            {copy.teleportApp}
          </Button>
        </div>
        {status ? (
          <p
            data-drop-status={status.kind}
            className={cn("px-2 pt-2 pb-1 text-center text-xs", status.kind === "failed" ? "text-destructive" : "text-muted-foreground")}
          >
            {status.text}
          </p>
        ) : null}
      </div>
    </section>
  );
}
