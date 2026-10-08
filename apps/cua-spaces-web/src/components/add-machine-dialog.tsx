// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { ExternalLinkIcon } from "lucide-react";
import type { ReactNode } from "react";

import { useSession, type HostFormField, type HostSetupGuide } from "@/bridge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogDescription, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { toast } from "@/components/ui/toast";

/** Where Cua Spaces is downloaded (`teleport::flow::CUA_INSTALL_URL`). */
export const INSTALL_URL = "https://cua.ai/install";

/** What each setup choice means for the other machine (the core's toggle help, `host::panel`). */
const CHOICE_HELP: Record<string, string> = {
  desktop: "Your devices can see and control its screen.",
  spare: "Its screen stays private. Your devices create Spaces on it.",
};

/**
 * "Add a machine": how host setup goes on the other computer, in the words
 * that computer will show (the core's `host.panel` and `host.formView`).
 * Setup runs there, so this only explains it. Its buttons call existing
 * bridge actions: `session.openExternal` and `session.signIn`.
 */
export function AddMachineDialog({ open, onOpenChange, guide }: { open: boolean; onOpenChange: (open: boolean) => void; guide: HostSetupGuide }) {
  const { data: session, openExternal, signIn } = useSession();
  const signedIn = session?.signedIn ?? false;
  const identity = session?.identity;
  const fail = (title: string) => (e: unknown) => toast(title, { description: e instanceof Error ? e.message : String(e) });
  const fields = guide.form?.fields ?? [];
  const basic = fields.filter((f) => !f.advanced);
  const advanced = fields.filter((f) => f.advanced);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogPopup className="top-[8vh] w-[min(600px,calc(100vw-2rem))]">
        <div className="max-h-[84vh] overflow-y-auto px-6 pt-5 pb-4">
          <DialogTitle className="text-[15px] font-semibold">Add a machine</DialogTitle>
          <DialogDescription className="mt-1 text-[13px] text-muted-foreground">
            Any Mac, Linux or Windows computer you own can run Spaces for you. You set it up on that computer, and it
            shows up here once it is online.
          </DialogDescription>

          <ol className="mt-5 space-y-5">
            <Step n={1} title="Install Cua Spaces on the other computer">
              <Button variant="outline" size="sm" className="mt-2" onClick={() => void openExternal(INSTALL_URL).catch(fail("Couldn't open the page"))}>
                Open download page <ExternalLinkIcon />
              </Button>
            </Step>

            <Step n={2} title="Sign in with the same account">
              {signedIn ? (
                <p>
                  Use {identity ? <span className="font-medium text-foreground">{identity}</span> : "the account you use here"}. Machines
                  join the account they are set up with.
                </p>
              ) : (
                <>
                  <p>You are not signed in here. Sign in so this app can list the machines on your account.</p>
                  <Button size="sm" className="mt-2" onClick={() => void signIn().catch(fail("Couldn't start sign-in"))}>
                    Sign in
                  </Button>
                </>
              )}
            </Step>

            <Step n={3} title="Open This machine and choose how to use it">
              <p>On the other computer, open This machine. It offers two ways to set it up:</p>
              <ul className="mt-2 divide-y overflow-hidden rounded-lg border bg-card">
                {guide.choices.map((c) => (
                  <li key={c.id} className="px-3 py-2">
                    <div className="text-[13px] text-foreground">{c.label}</div>
                    {CHOICE_HELP[c.id] ? <div className="text-xs">{CHOICE_HELP[c.id]}</div> : null}
                  </li>
                ))}
              </ul>
            </Step>

            <Step n={4} title={guide.form ? `Fill in the form and press ${quote(submitLabel(guide.form.submitLabel))}` : "Set it up for access"}>
              <p>It connects through the Cua relay, so you don't need to forward ports. Under Advanced you can choose a direct connection instead.</p>
              {basic.length ? (
                <dl className="mt-2 divide-y overflow-hidden rounded-lg border bg-card">
                  {basic.map((f) => (
                    <FieldRow key={f.id} field={f} />
                  ))}
                  {advanced.length ? (
                    <div className="bg-muted/40 px-3 py-1.5 text-xs font-medium">{guide.form?.advancedLabel ?? "Advanced"}</div>
                  ) : null}
                  {advanced.map((f) => (
                    <FieldRow key={f.id} field={f} />
                  ))}
                </dl>
              ) : null}
            </Step>

            <Step n={5} title="Grant permissions if it shares its desktop">
              <p>On a Mac, This machine lists the System Settings panes to turn on, such as Screen Recording and Accessibility.</p>
            </Step>
          </ol>
        </div>
        <div className="flex justify-end gap-2 border-t px-6 py-3">
          <Button onClick={() => onOpenChange(false)}>Done</Button>
        </div>
      </DialogPopup>
    </Dialog>
  );
}

function Step({ n, title, children }: { n: number; title: string; children: ReactNode }) {
  return (
    <li className="grid grid-cols-[24px_minmax(0,1fr)] gap-3">
      <span className="flex size-6 items-center justify-center rounded-full bg-muted text-xs font-medium text-muted-foreground tabular-nums">{n}</span>
      <div className="pt-0.5 text-[13px] text-muted-foreground">
        <h3 className="mb-1 font-medium text-foreground">{title}</h3>
        {children}
      </div>
    </li>
  );
}

/** One form field: its label, and what goes in it. */
function FieldRow({ field: f }: { field: HostFormField }) {
  const value = f.choices?.length
    ? f.choices.map((c, i) => (i ? c.label[0]!.toLowerCase() + c.label.slice(1) : c.label)).join(", or ")
    : f.toggle
      ? "Off unless you need it"
      : (f.placeholder ?? f.value);
  return (
    <div className="flex items-baseline justify-between gap-4 px-3 py-2">
      <dt className="shrink-0 text-[13px] text-foreground">{f.label}</dt>
      <dd className="min-w-0 truncate text-right text-xs">{value}</dd>
    </div>
  );
}

const quote = (s: string) => `“${s}”`;
/** "Set up for access" (the label while idle). */
const submitLabel = (s: string) => s.replace(/…$/, "");
