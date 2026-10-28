// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Settings sections share one gap, one heading style and a footer slot, so a
// link or a button under a card never needs a loose margin.

import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { SettingsGroup } from "@/components/settings-group";
import { GroupedAction } from "@/components/ui/grouped";

afterEach(cleanup);

describe("SettingsGroup", () => {
  it("gives every section the same 32px gap and the one uppercase heading", () => {
    const { container } = render(
      <>
        <SettingsGroup title="Labs">one</SettingsGroup>
        <SettingsGroup title="Connection">two</SettingsGroup>
      </>,
    );
    const sections = [...container.querySelectorAll("section")];
    expect(sections).toHaveLength(2);
    for (const s of sections) {
      expect(s.className).toContain("mb-8");
      expect(s.querySelector("h2")?.className).toContain("uppercase");
    }
  });

  it("holds a link under the card in the footer, and an action as the card's last row", () => {
    const { container } = render(
      <SettingsGroup title="Everyday" footer={<button type="button">What is collected</button>}>
        <GroupedAction>Show all</GroupedAction>
      </SettingsGroup>,
    );
    const card = container.querySelector("[data-grouped]")!;
    expect(card.lastElementChild?.textContent).toBe("Show all");
    expect(card.nextElementSibling?.textContent).toBe("What is collected");
  });
});
