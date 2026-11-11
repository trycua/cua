// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useNavigate } from "@tanstack/react-router";
import { useMemo, useState, type ComponentProps } from "react";

import { agentsView, useBridge, useMachines, useSpaces, type AgentAccount, type AgentGroup, type SpaceOs } from "@/bridge";
import { useChildren } from "@/components/children/use-children";
import { toast } from "@/components/ui/toast";
import { useAgentMenu } from "@/hooks/use-agent-menu";
import { connectFailureLine, useConnectors } from "@/hooks/use-connectors";
import { useT } from "@/i18n";
import { reusableHome } from "@/lib/agent-home/home-space";
import { currentWhere, thisMacConfirmed } from "@/lib/ask-first";
import { pickAccount, pickOf, rememberAgent, resolveAgent, usePickedAgent } from "@/lib/ask-first-model";
import { useRepos } from "@/lib/ask-first-repo";
import { homeDefaultWhere } from "@/lib/this-mac-target";

import { AskBox, type TaskWhere } from "./ask-box";
import { useWherePick } from "./this-mac-confirm";
import { agentName } from "./task-agents";
import { useStartTask } from "./use-start-task";
import { useThisMacTarget } from "./use-this-mac-target";
import { useWhereOptions } from "./where-options";

type BoxProps = ComponentProps<typeof AskBox>;

/**
 * The Ask first box and everything it needs: the job text, the agent and account, the repo, where it runs,
 * and starting the task. Home and the Agents tab draw the same box from this.
 */
export function useAskComposer() {
  const t = useT();
  const { data: spaces } = useSpaces();
  const { kinds, spaces: pickable, computers } = useWhereOptions();
  // This computer, once it's in the account, is where a job goes unless you pick (no Space needed first).
  const thisMac = useThisMacTarget();
  const { start } = useStartTask();
  const { core } = useBridge();
  // The agent: the last app, account and model picked while they are still there, else the first connected app. The
  // account is the app's own choice (kept by the host), so a run uses the one shown.
  const connectors = useConnectors();
  // Which apps the menu lists is the person's choice (Settings, Connections); an app they connect shows again.
  const menu = useAgentMenu(connectors);
  const navigate = useNavigate();
  const agents = useMemo(() => (core ? agentsView(core, connectors.input, menu.prefs) : null), [core, connectors.input, menu.prefs]);
  const picked = usePickedAgent();
  const selected = resolveAgent(agents, picked);
  const { current: repo } = useRepos();
  const [text, setText] = useState("");
  // With Child Spaces on (and this Home, the Ask first experiment), the job goes to the agent's Home Space, which
  // makes the Spaces it needs on your computers when asked in plain words.
  const children = useChildren();
  const agentHome = Boolean(children);
  const chosenAgent = selected?.harness ?? "claude-code";
  const { data: machineRows } = useMachines();
  const machines = useMemo(
    () =>
      (machineRows ?? []).map((m) => ({
        id: m.id,
        name: m.name,
        current: m.current,
        online: m.online,
        os: (m.os === "macos" || m.os === "linux" || m.os === "windows" ? m.os : "unknown") as SpaceOs,
      })),
    [machineRows],
  );
  // This computer once it's in the account, else a new Linux computer, unless you pick; a pick that's no longer
  // offered falls back to it.
  const [pickedWhere, setWhere] = useState<TaskWhere | null>(null);
  const [whereOpen, setWhereOpen] = useState(false);
  const pick = useWherePick(agentName(chosenAgent));
  const where = currentWhere(pickedWhere ?? (agentHome ? { kind: "agent" } : homeDefaultWhere(thisMac.readiness, kinds)), kinds, [...pickable, ...computers]);
  const submit = (prompt = text, w = where) => {
    if (!prompt.trim() || !selected) return;
    // The default can be This Mac itself: it asks once, as picking it does.
    if (w.kind === "this-mac" && thisMac.readiness === "needs-setup") return;
    if (w.kind === "this-mac" && !thisMacConfirmed()) {
      pick.choose(
        w,
        (ok) => submit(prompt, ok),
        () => setWhereOpen(true),
      );
      return;
    }
    // The run uses the app's own choice of account: make it the one shown.
    if (!selected.account.selected) void connectors.useCredential(selected.group.app, { env: selected.account.env, slot: selected.account.slot });
    start({ prompt, agent: chosenAgent, where: w, model: selected.model.id, ...(selected.effort ? { effort: selected.effort } : {}), repo });
    setText("");
  };
  // A sign-in that did not work says so, in plain words, where the person clicked.
  const signInTo = async (app: string, label: string) => {
    if ((await connectors.connectApp(app)) === "failed") {
      toast(t("home.ask_box.sign_in_failed", { label }), {
        description: connectFailureLine(app) || t("home.ask_box.sign_in_failed_hint"),
        type: "error",
        group: `sign-in:${app}`,
      });
    }
  };
  const useAccount = (group: AgentGroup, account: AgentAccount) => {
    rememberAgent(pickAccount(group, account, picked));
    if (!account.selected) void connectors.useCredential(group.app, { env: account.env, slot: account.slot });
  };
  const box: BoxProps = {
    text,
    onText: setText,
    view: agents,
    selected,
    onPickAccount: useAccount,
    onModel: (model) => selected && rememberAgent({ ...pickOf(selected), model }),
    onEffort: (effort) => selected && rememberAgent({ ...pickOf(selected), effort }),
    onSignIn: (g) => void signInTo(g.app, g.label),
    onConfigure: () => void navigate({ to: "/settings/connections" }),
    repo,
    where,
    onWhere: (w) => pick.choose(w, setWhere, () => setWhereOpen(true)),
    whereOpen,
    onWhereOpen: setWhereOpen,
    kinds,
    spaces: pickable,
    computers,
    thisMac: { readiness: thisMac.readiness, busy: thisMac.busy, onSetUp: thisMac.setUp },
    machines,
    agentHome,
    hasHome: agentHome && reusableHome(chosenAgent, spaces, false, children?.homesOf(chosenAgent)) !== null,
    busy: false,
    onSubmit: () => submit(),
  };
  return { box, dialog: pick.dialog, agents, selected, chosenAgent, agentHome, kinds, signInTo, setText, setWhere };
}

/** The Ask first box on its own, for a page that is not Home (the Agents tab's quick start). */
export function AskComposer({ compact = true, autoFocus }: { compact?: boolean; autoFocus?: boolean }) {
  const c = useAskComposer();
  return (
    <>
      <AskBox {...c.box} compact={compact} autoFocus={autoFocus} pickerSide="up" />
      {c.dialog}
    </>
  );
}
