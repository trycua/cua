// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { ChevronRightIcon, FolderIcon } from "lucide-react";
import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Dialog, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { GroupedSection, SettingRow } from "@/components/ui/grouped";
import { Segmented } from "@/components/ui/segmented";
import { Select } from "@/components/ui/select";

import { useNetworkPreview } from "./use-network";

type Access = "read" | "write";
type Duration = "off" | "hour" | "running";

/**
 * An agent in a Space asks for a folder of the Volume (demo data for now:
 * real requests are answered on the Volume page)
 * (modeled on Keyvault's approval sheet). Read only is the default; nothing
 * is given until Allow.
 */
export function VolumeGrantDialog({ spaceId, spaceName }: { spaceId: string; spaceName: string }) {
  const store = useNetworkPreview();
  const req = store?.volumeRequest(spaceId);
  const [access, setAccess] = useState<Access>("read");
  const [duration, setDuration] = useState<Duration>("off");
  if (!store || !req) return null;
  const answer = () => store.answerVolume(req.id);
  const [first, ...rest] = req.path.split("/");
  return (
    <Dialog open onOpenChange={(open) => !open && answer()}>
      <DialogPopup data-volume-grant="" className="w-[min(520px,calc(100vw-2rem))]">
        <div className="px-5 pt-5 pb-4">
          <DialogTitle className="text-[15px] font-semibold">Give {spaceName} access to a folder?</DialogTitle>
          <p className="mt-1 text-xs text-muted-foreground">
            {req.who} in {spaceName} asks to read {req.path}.
          </p>
          <GroupedSection className="mt-4">
            <SettingRow
              label="Folder"
              control={
                <span data-volume-grant-path="" className="inline-flex h-6 items-center gap-1 rounded-lg bg-muted px-2 text-xs">
                  <FolderIcon className="size-3.5 text-muted-foreground" />
                  Volume
                  <ChevronRightIcon className="size-3 text-muted-foreground" />
                  <span className="font-medium">{[first, ...rest].join("/")}</span>
                </span>
              }
            />
            <SettingRow
              label="Access"
              control={
                <Segmented
                  aria-label="Access"
                  value={access}
                  options={[
                    { value: "read", label: "Read only (recommended)" },
                    { value: "write", label: "Read and write" },
                  ]}
                  onValueChange={setAccess}
                />
              }
            />
            <SettingRow
              label="Duration"
              control={
                <Select
                  aria-label="Duration"
                  className="min-w-44"
                  value={duration}
                  options={[
                    { value: "off", label: "Until I turn it off" },
                    { value: "hour", label: "1 hour" },
                    { value: "running", label: "While the Space runs" },
                  ]}
                  onValueChange={setDuration}
                />
              }
            />
          </GroupedSection>
        </div>
        <div /* design-ok: full-width dialog footer */ className="flex items-center gap-2 border-t bg-muted/60 px-5 py-3">
          <span className="flex-1 text-xs text-muted-foreground">You can see and revoke this in the Space's details.</span>
          <Button variant="secondary" data-volume-grant-deny="" onClick={answer}>
            Deny
          </Button>
          <Button data-volume-grant-allow="" onClick={answer}>
            Allow
          </Button>
        </div>
      </DialogPopup>
    </Dialog>
  );
}
