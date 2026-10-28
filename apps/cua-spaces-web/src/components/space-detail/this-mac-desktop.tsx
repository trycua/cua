// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { MonitorIcon } from "lucide-react";
import { useMemo, useState, type ReactNode } from "react";

import type { SpaceFact, SpaceThread } from "@/bridge";
import { useDesktopSwitch } from "@/components/desktop-switch";
import { SpaceCall } from "@/components/space-thread/space-call";
import { Button } from "@/components/ui/button";
import { thisComputerText } from "@/lib/host-labels";
import { cn } from "@/lib/utils";

/** The row's thread on a daemon that has none: empty, read-only in effect. */
const EMPTY: SpaceThread = {
  status: "ready",
  items: [],
  entries: [],
  members: [],
  shares: [],
  me: "me",
  role: null,
  error: null,
  hasMoreBefore: false,
  loadEarlier: async () => {},
  post: async () => null,
  answer: async () => {},
  setRights: async () => {},
  removeAgent: null,
};

/** Said when a message is written while the desktop is not shared, and kept in the box. */
export const NOT_SHARING_CHAT = "Share this desktop to chat here.";

/**
 * The stage of This Mac's desktop page. It is never a stream (the screen in front of you would only
 * mirror itself): while the desktop is not shared it says so, with one button that turns sharing on;
 * while it is, it says that instead. `nudge` lights it when someone writes without sharing.
 */
export function DesktopStage({ shared, nudge }: { shared: boolean; nudge?: boolean }) {
  const { toggle, busy, flip } = useDesktopSwitch();
  const here = thisComputerText();
  return (
    <div
      data-this-mac-desktop={shared ? "shared" : "off"}
      data-nudge={nudge ? "" : undefined}
      className={cn("flex h-full min-h-48 flex-col items-center justify-center gap-3 rounded-lg bg-tile px-6 text-center", nudge && "ring-2 ring-brand")}
    >
      <MonitorIcon className="size-6 text-muted-foreground/50" aria-hidden />
      <div>
        <h2 className="text-[15px] font-semibold">{shared ? "You're sharing your desktop as a Space" : "You aren't sharing your desktop as a Space"}</h2>
        <p className="mt-1 text-[13px] text-muted-foreground">
          {shared ? `Your other machines can see and control the screen of ${here}. It isn't shown here.` : `Turn this on to see and control the screen of ${here} from your other machines.`}
        </p>
      </div>
      {toggle ? (
        <Button variant={shared ? "outline" : "default"} disabled={!toggle.editable || !toggle.enabled || busy} onClick={flip} data-desktop-share-button="">
          {shared ? "Stop sharing" : "Share this desktop"}
        </Button>
      ) : null}
    </div>
  );
}

/**
 * This Mac's desktop row opened: a Space's page as any other (the header, the thread with its composer,
 * the rail), with the prompt where the stream would be. The thread is the daemon's own thread of this
 * machine's desktop row (`live`): writing while the desktop is not shared lights the prompt and keeps the
 * draft; once it is shared, messages are posted to the thread as in any Space.
 */
export function ThisMacCall({
  spaceName,
  shared,
  header,
  facts,
  more,
  identity,
  live,
}: {
  spaceName: string;
  shared: boolean;
  header: ReactNode;
  facts: readonly SpaceFact[];
  more: ReactNode;
  identity: string | null;
  /** The row's thread from the daemon; `unsupported` from a daemon that has none yet. */
  live: SpaceThread;
}) {
  const [nudge, setNudge] = useState(0);
  const noThread = live.status === "unsupported";
  const thread = useMemo<SpaceThread>(
    () => ({
      ...(noThread ? EMPTY : live),
      // Not shared: the prompt lights up and the draft stays in the box.
      post: async (text, view) => {
        if (!shared) {
          setNudge((n) => n + 1);
          throw new Error(NOT_SHARING_CHAT);
        }
        if (noThread) throw new Error("Chat isn't available on this Mac's own desktop yet.");
        return live.post(text, view);
      },
    }),
    [shared, live, noThread],
  );
  return (
    <SpaceCall
      spaceId="this-mac"
      spaceName={spaceName}
      identity={identity}
      header={header}
      stage={<DesktopStage shared={shared} nudge={nudge > 0 && !shared} key={nudge} />}
      thread={thread}
      // The composer posts straight to this thread (no summon behind it).
      creating
      facts={facts}
      more={more}
      windows={[]}
      desktop={null}
      onStream={() => {}}
    />
  );
}
