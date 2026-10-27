// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { DesktopSwitchRow } from "@/components/desktop-switch";
import { GroupedSection } from "@/components/ui/grouped";
import { thisComputerText } from "@/lib/host-labels";

/**
 * The page of This Mac's desktop row. No stream: it is the screen in front of you, so a stream
 * only mirrors itself. It says whether the desktop is shared as a Space, why that matters in one
 * line, and holds the switch.
 */
export function ThisMacDesktop({ shared }: { shared: boolean }) {
  const here = thisComputerText();
  return (
    <section data-this-mac-desktop={shared ? "shared" : "off"} className="max-w-[620px]">
      <h2 className="text-[17px] font-semibold">{shared ? "You're sharing your desktop as a Space" : "You aren't sharing your desktop as a Space"}</h2>
      <p className="mt-1 text-[13px] text-muted-foreground">
        {shared ? `Your other machines can see and control the screen of ${here}.` : `Turn this on to see and control the screen of ${here} from your other machines.`}
      </p>
      <GroupedSection className="mt-5">
        <DesktopSwitchRow />
      </GroupedSection>
    </section>
  );
}
