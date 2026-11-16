// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * What a person gives an agent on another machine so it can use this one: the host to name, the
 * exact commands, the permissions and the reload note. One place, so the Connect dialog and the
 * first run say the same thing.
 */

import { t } from "@/i18n";

/** The Space an agent on another machine names to use this machine's desktop (`relay:<machine id>`). */
export const desktopSpace = (relayId: string) => `relay:${relayId}`;

/** The agent ids `cua agents setup --agents` takes, by the ids the app uses. */
const SETUP_ID = { "claude-code": "claude", codex: "codex", hermes: "hermes", openclaw: "openclaw" } as const;

/** The commands for `agent`, run on the other machine (not this one): the same account, then Cua's server and skills in the agent. */
export function otherMachineCommand(agent: keyof typeof SETUP_ID): string {
  return `cua auth login\ncua agents setup --agents ${SETUP_ID[agent]} --yes`;
}

/** What to tell the agent once it has the server: which machine, which Space. */
export function otherMachinePrompt(name: string, relayId: string | undefined): string {
  return relayId
    ? `Use Cua to control "${name}": take a screenshot with computer_screenshot and space="${desktopSpace(relayId)}".`
    : `Use Cua to control "${name}": list the computers with list_spaces and use its Space.`;
}

/** The permissions line: what the machine must allow, and how to give the agent less. */
/** @deprecated English only: use {@link otherMachinePermissions}. */
export const OTHER_MACHINE_PERMISSIONS =
  "This computer has to share its desktop (its page in Computers, Let other computers control this desktop) and be online. The agent gets every Cua tool by default; add --permissions computer:all to cua mcp to give it only the computer tools.";

export const otherMachinePermissions = (): string => t("computers.row.other_permissions");

/** @deprecated English only: use {@link otherMachineReload}. */
export const OTHER_MACHINE_RELOAD = "Restart the agent after adding the server so it loads the tools.";

/** Why a reload is needed. */
export const otherMachineReload = (): string => t("computers.row.other_reload");
