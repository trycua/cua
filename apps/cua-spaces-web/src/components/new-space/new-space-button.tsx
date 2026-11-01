// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { ChevronDownIcon, PlusIcon } from "lucide-react";

import { useNewSpaceWizard } from "@/bridge";
import { Button } from "@/components/ui/button";
import { Menu } from "@/components/ui/menu";
import { useNewSpace } from "@/hooks/use-new-space";

/**
 * "New Space", with an arrow for the other way to add something to run on:
 * a machine by its address (it does not use the image picker). Where the
 * wizard cannot run (the host keeps New Space native), the button stands alone.
 */
export function NewSpaceButton() {
  const newSpace = useNewSpace();
  const wizard = useNewSpaceWizard();
  if (!wizard.offered) {
    return (
      <Button onClick={newSpace}>
        <PlusIcon /> New Space
      </Button>
    );
  }
  return (
    <div className="inline-flex" data-new-space-split="">
      <Button
        onClick={newSpace}
        className="rounded-r-none" // design-ok: the two halves of one split button
      >
        <PlusIcon /> New Space
      </Button>
      <Menu
        trigger={
          <Button
            aria-label="More ways to add"
            title="More ways to add"
            data-new-space-more=""
            className="w-7 rounded-l-none border-l-primary-foreground/25 px-0" // design-ok: the two halves of one split button
          >
            <ChevronDownIcon className="size-3.5" />
          </Button>
        }
        items={[{ label: "Add a machine by its address\u2026", onSelect: () => wizard.showAddress() }]}
      />
    </div>
  );
}
