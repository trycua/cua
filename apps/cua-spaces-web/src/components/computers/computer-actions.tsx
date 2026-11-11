// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useNavigate } from "@tanstack/react-router";
import { useState, type ReactNode } from "react";

import { useMachines, useSession, useThisMachine } from "@/bridge";
import type { HeaderAction } from "@/components/space-detail/space-header";
import { useSpacesTreeData } from "@/components/spaces/use-spaces-tree";
import { ConfirmDialog } from "@/components/ui/alert-dialog";
import { toast } from "@/components/ui/toast";
import { t, useT } from "@/i18n";
import { computerParam } from "@/lib/machines";

const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e));

/**
 * The Remove prompt, by whose computer it is: the one you are on offers Stop sharing and Stop sharing and sign out while
 * it shares, and only Sign out once it does not; another computer is one confirmation.
 */
export function removePrompt({ current, sharing, name }: { current: boolean; sharing: boolean; name: string }): {
  title: string;
  body: string;
  confirm: string;
  alternative: string | null;
  destructive: boolean;
} {
  if (current && sharing)
    return {
      title: t("computers.tree.remove_title", { name }),
      body: t("computers.page.remove_own_body", { name }),
      confirm: t("computers.page.stop_sharing"),
      alternative: t("computers.page.stop_sharing_and_sign_out"),
      destructive: false,
    };
  if (current)
    return {
      title: t("computers.page.sign_out_title"),
      body: t("computers.page.sign_out_body_here"),
      confirm: t("shell.account.sign_out"),
      alternative: null,
      destructive: true,
    };
  return {
    title: t("computers.page.sign_out_title"),
    body: t("computers.page.sign_out_body_other", { name }),
    confirm: t("computers.account.remove"),
    alternative: null,
    destructive: true,
  };
}

/**
 * What managing a computer needs on the page its row opens (its desktop's Space page): its own page (switches,
 * rename, details) in the header menu, and Remove (the trash). On the computer you are on, Remove offers Stop sharing and
 * Stop sharing and sign out (only Sign out once it is not shared); on another computer it is one confirmation, kept off
 * while that computer is online (the relay refuses it). Stopping sharing is also the page's own button.
 * Null for a Space that is no computer's desktop.
 */
export function useComputerActions(spaceId: string): { settings: HeaderAction; remove: HeaderAction; dialogs: ReactNode } | null {
  const t = useT();
  const navigate = useNavigate();
  const { tree } = useSpacesTreeData();
  const machines = useMachines();
  const { data: session, signOut } = useSession();
  const host = useThisMachine(session?.identity ?? null);
  const [asking, setAsking] = useState(false);
  const node = tree?.roots.find((r) => r.kind === "computer" && r.machine && r.opens === spaceId) ?? null;
  if (!node?.machine) return null;
  const machine = node.machine;
  const view = machines.view?.machines.find((m) => m.id === machine.id) ?? null;
  const name = node.name;
  const r = view?.remove ?? null;
  const blocked = !node.current && r ? !r.enabled && node.online : false;
  const fail = (what: string) => (e: unknown) => toast(what, { description: errorText(e), type: "error" });
  const settings: HeaderAction = {
    label: t("computers.page.computer_settings"),
    enabled: true,
    onClick: () => void navigate({ to: "/spaces/computer/$computerId", params: { computerId: computerParam(machine) } }),
  };
  const remove: HeaderAction = {
    label: t("computers.tree.remove_named", { name }),
    help: blocked ? (r?.help ?? undefined) : undefined,
    enabled: !blocked,
    onClick: () => setAsking(true),
  };
  const sharing = Boolean(view && view.status !== t("computers.row.not_sharing"));
  // Signed out, this computer's page is no longer yours: back to the list.
  const signOutNow = () => void signOut().then(() => void navigate({ to: "/spaces" }), fail(t("shell.account.sign_out_failed")));
  const stopSharing = () => host.run("stop-sharing").catch(fail(t("computers.page.stop_sharing_failed", { name })));
  const removeOther = () => {
    void navigate({ to: "/spaces" });
    void machines
      .remove(r?.machine ?? machine.id, r?.device ?? null)
      .then(() => toast(t("computers.page.removed", { name }), { type: "success" }), fail(t("computers.page.remove_failed", { name })));
  };
  const text = removePrompt({ current: node.current, sharing, name });
  const ask = {
    ...text,
    onConfirm: node.current ? (sharing ? () => void stopSharing() : signOutNow) : removeOther,
    alternative: text.alternative ? { label: text.alternative, onClick: () => void stopSharing().then(signOutNow) } : null,
  };
  const dialogs = (
    <ConfirmDialog
      open={asking}
      onOpenChange={setAsking}
      title={ask.title}
      description={ask.body}
      confirmLabel={ask.confirm}
      destructive={ask.destructive}
      alternative={ask.alternative}
      onConfirm={ask.onConfirm}
    />
  );
  return { settings, remove, dialogs };
}
