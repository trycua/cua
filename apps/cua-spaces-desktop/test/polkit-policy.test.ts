// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The polkit actions the .deb installs (packaging/ai.cua.spaces.policy).
// polkitd skips a policy file it cannot parse, and then no prompt can be
// asked for: it must be well-formed XML, read here by a strict parser.
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import * as path from "node:path";
import { describe, expect, it } from "vitest";

const require = createRequire(import.meta.url);
const sax = require("sax") as {
  parser(strict: true): {
    onerror: (e: Error) => void;
    onopentag: (tag: { name: string; attributes: Record<string, string> }) => void;
    ontext: (text: string) => void;
    onclosetag: (name: string) => void;
    write(xml: string): { close(): void };
  };
};

const policyFile = path.join(import.meta.dirname, "../packaging/ai.cua.spaces.policy");

/** Every action's id, message and defaults; throws on the first XML error. */
function parsePolicy(xml: string): Map<string, Record<string, string>> {
  const parser = sax.parser(true);
  const actions = new Map<string, Record<string, string>>();
  let action: Record<string, string> | null = null;
  let text = "";
  parser.onerror = (e) => {
    throw e;
  };
  parser.onopentag = (tag) => {
    text = "";
    if (tag.name === "action") {
      action = {};
      actions.set(tag.attributes.id ?? "", action);
    }
  };
  parser.ontext = (t) => {
    text += t;
  };
  parser.onclosetag = (name) => {
    if (name === "action") action = null;
    else if (action) action[name] = text.trim();
  };
  parser.write(xml).close();
  return actions;
}

describe("the polkit policy", () => {
  it("is well-formed XML (a comment may not hold two hyphens in a row)", () => {
    expect(() => parsePolicy(readFileSync(policyFile, "utf8"))).not.toThrow();
    expect(() => parsePolicy("<policyconfig><!-- pkcheck --action-id --></policyconfig>")).toThrow(/comment/i);
  });

  it("asks the user themself, every time, for the Keyvault and for approving a device", () => {
    const actions = parsePolicy(readFileSync(policyFile, "utf8"));
    expect([...actions.keys()]).toEqual(["ai.cua.spaces.keyvault", "ai.cua.spaces.devices"]);
    for (const a of actions.values()) {
      expect(a).toMatchObject({ allow_any: "no", allow_inactive: "no", allow_active: "auth_self" });
      // polkit fills placeholders only from details, which a non-root caller may not pass.
      expect(a.message).not.toMatch(/\$\(/);
      expect(a.message).not.toBe("");
    }
  });

  it("is installed by the .deb where polkitd reads actions", () => {
    const config = require("../electron-builder.config.cjs");
    expect(config.deb.fpm).toContain(`${policyFile}=/usr/share/polkit-1/actions/ai.cua.spaces.policy`);
  });
});
