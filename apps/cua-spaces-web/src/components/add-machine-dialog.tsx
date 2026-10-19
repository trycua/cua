// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { CheckIcon, ChevronRightIcon, CopyIcon, ExternalLinkIcon, LoaderCircleIcon } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import { encode } from "uqr";

import { expiresInText, FINISHING_SETUP, teamName, useMachines, useTeamChoices, usePairingCode, useSession, type HostFormField, type HostSetupGuide, type Machine, type PairingHook } from "@/bridge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogDescription, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { Segmented } from "@/components/ui/segmented";
import { Select } from "@/components/ui/select";
import { toast } from "@/components/ui/toast";
import { cn } from "@/lib/utils";

/** Where Cua Spaces is downloaded (`teleport::flow::CUA_INSTALL_URL`). */
export const INSTALL_URL = "https://cua.ai/install";

/** What each setup choice means for the other machine (the core's toggle help, `host::panel`). */
const CHOICE_HELP: Record<string, string> = {
  desktop: "Your devices can see and control its screen.",
  spare: "Its screen stays private. Your devices create Spaces on it.",
};

type Shell = "sh" | "powershell";
const SHELLS = [
  { value: "sh", label: "macOS / Linux" },
  { value: "powershell", label: "Windows (preview)" },
] as const satisfies readonly { value: Shell; label: string }[];

/**
 * "Add a machine". Signed in on an enrolled device, opening it makes a
 * one-time link (and QR code, and install commands) that adds the other
 * computer to the account, and waits until that computer uses it. Without
 * one (signed out, not enrolled, a relay that can't make links), it explains
 * setting the other computer up by hand, in the words that computer shows
 * (the core's `host.panel` and `host.formView`).
 */
export function AddMachineDialog({
  open,
  onOpenChange,
  guide,
  pollMs,
  team = null,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  guide: HostSetupGuide;
  /** How often to ask whether the link was used (tests). */
  pollMs?: number;
  /** The team (organization id) chosen at first; default: the current team, when it can take machines. */
  team?: string | null;
}) {
  const { data: session, signIn } = useSession();
  const { data: listed, refresh } = useMachines();
  const machines = listed ?? [];
  const signedIn = session?.signedIn ?? false;
  // Signed in but not enrolled: this device can't vouch for a new machine
  // (known once the machines are read).
  const notice = machines.find((m) => m.current)?.accessNotice ?? null;
  // Which team the new machine joins (none: just this account). The
  // account's teams that can take machines, the current one first.
  const choices = useTeamChoices(open && signedIn);
  const [picked, setPicked] = useState<string | null | undefined>(undefined);
  useEffect(() => {
    if (!open) setPicked(undefined);
  }, [open]);
  const chosen = picked !== undefined ? picked : (team ?? choices.current);
  const pairing = usePairingCode({ open, enabled: signedIn && listed !== undefined && !notice, pollMs, team: chosen, onUsed: () => void refresh().catch(() => {}) });
  const fail = (title: string) => (e: unknown) => toast(title, { description: e instanceof Error ? e.message : String(e) });

  // Any reason there is no link: the manual steps are the way, so show them.
  const manualOnly = !signedIn || Boolean(notice) || pairing.phase === "error";

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogPopup className="top-[8vh] w-[min(600px,calc(100vw-2rem))]" data-add-machine={pairing.phase}>
        <div className="max-h-[84vh] overflow-y-auto px-6 pt-5 pb-4">
          <DialogTitle className="text-[15px] font-semibold">Add a computer</DialogTitle>
          <DialogDescription className="mt-1 text-[13px] text-muted-foreground">
            {manualOnly
              ? "Any Mac, Linux or Windows computer you own can run Spaces for you. You set it up on that computer, and it shows up here once it is online."
              : "Open this link on the other computer, scan the code with its camera, or run the command in its terminal. It installs Cua and adds the computer to your account."}
          </DialogDescription>

          {!signedIn ? (
            <Notice>
              <p>Sign in to add a computer with a link. You can also set it up by hand.</p>
              <Button size="sm" className="mt-2" onClick={() => void signIn().catch(fail("Couldn't start sign-in"))}>
                Sign in
              </Button>
            </Notice>
          ) : notice ? (
            <Notice>{notice.text || "This device isn't enrolled yet, so it can't make a link."} Set the computer up by hand instead.</Notice>
          ) : pairing.phase === "error" ? (
            <Notice>
              {pairing.error?.unsupported
                ? "Your Cua relay can't make links yet. Set the computer up by hand instead."
                : `${pairing.error?.message ?? "Couldn't make a link."} Set the computer up by hand instead.`}
              <Button variant="outline" size="sm" className="mt-2 flex" onClick={pairing.renew}>
                Try again
              </Button>
            </Notice>
          ) : (
            <>
              {choices.teams.length > 0 && pairing.phase !== "used" ? (
                <div className="mt-4 flex items-center gap-2 text-[13px]" data-add-machine-team={chosen ?? "me"}>
                  <span className="text-muted-foreground">Add it for</span>
                  <Select
                    aria-label="Add it for"
                    className="h-8 min-w-40"
                    value={chosen ?? "me"}
                    options={[
                      { value: "me", label: "Just me" },
                      ...choices.teams.map((t) => ({ value: t.id, label: `${teamName(t)} (team)` })),
                    ]}
                    onValueChange={(v) => setPicked(v === "me" ? null : v)}
                  />
                </div>
              ) : null}
              <PairingPanel pairing={pairing} machines={machines} />
            </>
          )}

          {pairing.phase === "used" ? null : <ManualSteps guide={guide} open={manualOnly} />}
        </div>
        <div className="flex justify-end gap-2 border-t px-6 py-3">
          {pairing.phase === "ready" ? (
            <Button variant="outline" onClick={() => void pairing.revoke().catch(fail("Couldn't revoke the link"))}>
              Revoke link
            </Button>
          ) : null}
          <Button onClick={() => onOpenChange(false)}>Done</Button>
        </div>
      </DialogPopup>
    </Dialog>
  );
}

function Notice({ children }: { children: ReactNode }) {
  return <div className="mt-4 rounded-lg border bg-card px-3 py-2.5 text-[13px] text-muted-foreground">{children}</div>;
}

/** The link, its QR code, the commands, and how long it lasts. */
function PairingPanel({ pairing, machines }: { pairing: PairingHook; machines: Machine[] }) {
  const [shell, setShell] = useState<Shell>("sh");
  const { phase, code } = pairing;

  if (phase === "idle" || phase === "minting" || !code) {
    return (
      <div className="mt-5 flex items-center gap-2 text-[13px] text-muted-foreground" data-pairing-minting>
        <LoaderCircleIcon className="size-3.5 animate-spin" aria-hidden /> Making a link…
      </div>
    );
  }

  if (phase === "used") {
    const name = machines.find((m) => m.id === code.machine)?.name;
    const team = pairing.team;
    if (team?.phase === "adding") {
      return (
        <div className="mt-5 flex items-center gap-2 text-[13px] text-muted-foreground" data-pairing-team="adding">
          <LoaderCircleIcon className="size-3.5 animate-spin" aria-hidden /> {FINISHING_SETUP}
        </div>
      );
    }
    return (
      <div className="mt-5 flex items-center gap-2.5 rounded-lg border bg-card px-3 py-3 text-[13px]" data-pairing-added data-pairing-team={team?.phase}>
        <CheckIcon className="size-4 shrink-0 text-success" aria-hidden />
        <span>
          {name ? `Added ${name}.` : "Added the computer."}{" "}
          {team?.phase === "added"
            ? "Your team can use it now."
            : team?.phase === "failed"
              ? `It isn't on your team yet: ${team.error ?? "something went wrong"}`
              : "It shows in your computers."}
        </span>
      </div>
    );
  }

  if (phase === "expired" || phase === "revoked") {
    return (
      <div className="mt-5 rounded-lg border bg-card px-3 py-3 text-[13px] text-muted-foreground" data-pairing-ended={phase}>
        {phase === "expired" ? "This link expired." : "This link was revoked. It can't add a computer now."}
        <Button size="sm" className="mt-2 flex" onClick={pairing.renew}>
          Make a new link
        </Button>
      </div>
    );
  }

  const command = shell === "sh" ? code.installSh : code.installPowershell;
  return (
    <div className="mt-5" data-pairing-ready>
      <div className="grid grid-cols-[auto_minmax(0,1fr)] items-start gap-5">
        {code.url ? <QrCode text={code.url} /> : null}
        <div className="min-w-0 space-y-3">
          <div>
            <div className="mb-1 text-xs font-medium text-muted-foreground">Link</div>
            {code.url ? <CopyField text={code.url} label="Copy link" /> : null}
          </div>
          {code.code ? (
            <div data-pairing-code>
              <div className="mb-1 text-xs font-medium text-muted-foreground">Code</div>
              <CopyField text={code.code} label="Copy code" mono />
              <p className="mt-1.5 text-xs text-muted-foreground">Already have Cua Spaces there? Open Computers, choose Join with a code and enter it.</p>
            </div>
          ) : null}
          <div>
            <Segmented aria-label="The other computer's system" value={shell} options={SHELLS} onValueChange={setShell} />
            {command ? <CopyField text={command} label="Copy command" mono /> : null}
          </div>
        </div>
      </div>
      <div className="mt-4 flex items-center justify-between gap-3 text-xs text-muted-foreground">
        <span className="flex items-center gap-2" data-pairing-waiting>
          <LoaderCircleIcon className="size-3.5 animate-spin" aria-hidden /> Waiting for the computer…
        </span>
        <span className="tabular-nums" data-pairing-expires>
          Works once. {expiresInText(code.expiresAt, pairing.now)}
        </span>
      </div>
    </div>
  );
}

/** One line to copy: the link, or a command. */
function CopyField({ text, label, mono }: { text: string; label: string; mono?: boolean }) {
  const [copied, setCopied] = useState(false);
  const copy = () => {
    void navigator.clipboard?.writeText(text).then(
      () => {
        setCopied(true);
        setTimeout(() => setCopied(false), 1200);
      },
      () => {},
    );
  };
  return (
    <div className="mt-1.5 flex items-center gap-1 rounded-lg border bg-muted/40 pl-3">
      <code className={cn("min-w-0 flex-1 py-1.5 text-xs", mono ? "font-mono break-all" : "truncate font-sans")} title={text}>
        {text}
      </code>
      <Button size="icon-sm" variant="ghost" aria-label={label} onClick={copy}>
        {copied ? <CheckIcon className="size-3.5" /> : <CopyIcon className="size-3.5" />}
      </Button>
    </div>
  );
}

/** The link as a QR code, drawn here (nothing leaves the page). */
export function QrCode({ text, size = 148 }: { text: string; size?: number }) {
  const qr = encode(text, { ecc: "M", border: 2 });
  let d = "";
  qr.data.forEach((row, y) =>
    row.forEach((dark, x) => {
      if (dark) d += `M${x} ${y}h1v1h-1z`;
    }),
  );
  return (
    <svg
      role="img"
      aria-label="QR code of the link"
      data-pairing-qr
      width={size}
      height={size}
      viewBox={`0 0 ${qr.size} ${qr.size}`}
      shapeRendering="crispEdges"
      className="rounded-lg border bg-white"
    >
      <path d={d} fill="#000" />
    </svg>
  );
}

/** The by-hand setup: install, sign in, set up for access. */
function ManualSteps({ guide, open }: { guide: HostSetupGuide; open: boolean }) {
  const { data: session, openExternal } = useSession();
  const identity = session?.identity;
  const fail = (title: string) => (e: unknown) => toast(title, { description: e instanceof Error ? e.message : String(e) });
  const fields = guide.form?.fields ?? [];
  const basic = fields.filter((f) => !f.advanced);
  const advanced = fields.filter((f) => f.advanced);

  const steps = (
    <ol className="mt-4 space-y-5">
      <Step n={1} title="Install Cua Spaces on the other computer">
        <Button variant="outline" size="sm" className="mt-2" onClick={() => void openExternal(INSTALL_URL).catch(fail("Couldn't open the page"))}>
          Open download page <ExternalLinkIcon />
        </Button>
      </Step>

      <Step n={2} title="Sign in with the same account">
        <p>
          Use {identity ? <span className="font-medium text-foreground">{identity}</span> : "the account you use here"}. Computers join the
          account they are set up with.
        </p>
      </Step>

      <Step n={3} title="Open This computer and choose how to use it">
        <ul className="mt-1 divide-y overflow-hidden rounded-lg border bg-card">
          {guide.choices.map((c) => (
            <li key={c.id} className="px-3 py-2">
              <div className="text-[13px] text-foreground">{c.label}</div>
              {CHOICE_HELP[c.id] ? <div className="text-xs">{CHOICE_HELP[c.id]}</div> : null}
            </li>
          ))}
        </ul>
      </Step>

      <Step n={4} title={guide.form ? `Fill in the form and press ${quote(submitLabel(guide.form.submitLabel))}` : "Set it up for access"}>
        <p>It connects through the Cua relay, so you don't need to forward ports.</p>
        {basic.length ? (
          <dl className="mt-2 divide-y overflow-hidden rounded-lg border bg-card">
            {basic.map((f) => (
              <FieldRow key={f.id} field={f} />
            ))}
            {advanced.length ? <div className="bg-muted/40 px-3 py-1.5 text-xs font-medium">{guide.form?.advancedLabel ?? "Advanced"}</div> : null}
            {advanced.map((f) => (
              <FieldRow key={f.id} field={f} />
            ))}
          </dl>
        ) : null}
      </Step>

      <Step n={5} title="Grant permissions if it shares its desktop">
        <p>On a Mac, This computer lists the System Settings panes to turn on, such as Screen Recording and Accessibility.</p>
      </Step>
    </ol>
  );

  if (open) return <div data-manual-steps>{steps}</div>;
  return (
    <details className="group mt-5 border-t pt-3" data-manual-steps>
      <summary className="flex cursor-default list-none items-center gap-1 text-[13px] text-muted-foreground hover:text-foreground [&::-webkit-details-marker]:hidden">
        <ChevronRightIcon className="size-3.5 transition-transform group-open:rotate-90" aria-hidden /> Set it up by hand instead
      </summary>
      {steps}
    </details>
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
