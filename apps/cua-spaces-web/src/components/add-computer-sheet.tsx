// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { CheckIcon, ChevronLeftIcon, ChevronRightIcon, LoaderCircleIcon, MoreHorizontalIcon } from "lucide-react";
import { Fragment, useEffect, useRef, useState, type CSSProperties, type FormEvent, type ReactNode } from "react";
import { encode } from "uqr";

import {
  displayPairingCode,
  expiresInText,
  FINISHING_SETUP,
  normalizePairingCode,
  pairingLinkRelay,
  teamName,
  useFlag,
  useJoinWithCode,
  useMachines,
  usePairingCode,
  usePlanLimitTelemetry,
  useRelayUsage,
  useSession,
  useTeamChoices,
  type Machine,
  type PairingHook,
} from "@/bridge";
import { CopyButton } from "@/components/prompt-line";
import { Button } from "@/components/ui/button";
import { GroupedSection } from "@/components/ui/grouped";
import { Dialog, DialogDescription, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Menu } from "@/components/ui/menu";
import { Segmented } from "@/components/ui/segmented";
import { Select } from "@/components/ui/select";
import { UpgradeCard } from "@/components/upgrade-card";
import { cn } from "@/lib/utils";

/** Where Cua Spaces is downloaded; the page picks the download for the computer that opens it. */
export const INSTALL_URL = "https://cua.ai/install";

/** How often the sheet reads the account's computers while it waits for a new one (ms). */
export const NEW_COMPUTER_POLL_MS = 3000;

/** How long after opening, computers that show up are the account's own, not new (ms). */
export const SETTLE_MS = 10_000;

/** The sheet's pages: the method rows, one page per method, and entering a code made elsewhere. */
export type AddView = "pick" | "signin" | "link" | "qr" | "command" | "code" | "enter";

type Shell = "sh" | "powershell";
const SHELLS = [
  { value: "sh", label: "macOS / Linux" },
  { value: "powershell", label: "Windows (preview)" },
] as const satisfies readonly { value: Shell; label: string }[];

/** The methods, in order; the first is the default. `code`: it needs a one-time code. */
export const METHODS: readonly { view: Exclude<AddView, "pick" | "enter">; label: string; code: boolean }[] = [
  { view: "signin", label: "Sign in there", code: false },
  { view: "link", label: "Open a link", code: true },
  { view: "qr", label: "Scan a QR code", code: true },
  { view: "command", label: "Run a command", code: true },
  { view: "code", label: "Enter a code", code: true },
];

const TITLES: Record<AddView, string> = {
  pick: "Add a computer",
  signin: "Sign in there",
  link: "Open a link",
  qr: "Scan a QR code",
  command: "Run a command",
  code: "Enter a code",
  enter: "Enter a code",
};

/**
 * The prompt for an agent on the other computer: one line, one link. The
 * link's page tells the agent how to install with the code. `url` is the
 * link the core made (it keeps a non-default relay); without it, the plain
 * add link for the code.
 */
export function agentPrompt(code: string, url: string | null = null): string {
  return `Add this computer to my Cua Spaces: ${url ?? `https://cua.ai/add/${code}`}`;
}

/**
 * "Add a computer": a title, one line, and one row per method (Sign in
 * there, Open a link, Scan a QR code, Run a command, Enter a code), each
 * opening only what that method needs, and a prompt to paste into an agent
 * on the other computer. The footer waits for the computer and says
 * "Added: <name>" by itself, whichever way it was added. "Have a code?
 * Enter it" is the other side: this computer joins with a code made
 * elsewhere (a `cua://add/<code>` link opens it with the code filled in).
 *
 * A code is made on opening when this computer can make one (signed in and
 * enrolled); it renews by itself when it expires while the sheet is open.
 */
export function AddComputerSheet({
  open,
  onOpenChange,
  initialView = "pick",
  initialCode = "",
  initialRelay = null,
  pollMs,
  newComputerPollMs = NEW_COMPUTER_POLL_MS,
  settleMs = SETTLE_MS,
  team = null,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** The page shown on opening (`enter` for a `cua://add` link). */
  initialView?: AddView;
  /** A code from a `cua://add/<code>` link: fills Enter a code; nothing joins until the person presses Add. */
  initialCode?: string;
  initialRelay?: string | null;
  /** How often to ask whether the code was used (tests). */
  pollMs?: number;
  /** How often to read the computers while waiting (tests). */
  newComputerPollMs?: number;
  /** How long after opening a computer that shows up still counts as known (tests). */
  settleMs?: number;
  /** The team (organization id) chosen at first; default: the current team, when it can take computers. */
  team?: string | null;
}) {
  const { data: session, signIn } = useSession();
  const { data: listed, refresh } = useMachines();
  const machines = listed ?? [];
  const signedIn = session?.signedIn ?? false;
  const notice = machines.find((m) => m.current)?.accessNotice ?? null;
  const [view, setView] = useState<AddView>(initialView);
  useEffect(() => {
    if (open) setView(initialView);
  }, [open, initialView]);

  // Which team the new computer joins (none: just this account): only
  // teams on the Teams plan where this account is an owner or admin.
  const choices = useTeamChoices(open && signedIn);
  const [picked, setPicked] = useState<string | null | undefined>(undefined);
  useEffect(() => {
    if (!open) setPicked(undefined);
  }, [open]);
  const chosen = picked !== undefined ? picked : (team ?? choices.current);
  const chosenTeam = chosen ? choices.all.find((t) => t.id === chosen) : undefined;

  const canCode = signedIn && listed !== undefined && !notice;
  const pairing = usePairingCode({ open, enabled: canCode, pollMs, team: chosen, onUsed: () => void refresh().catch(() => {}) });
  const noCode = !signedIn || Boolean(notice) || pairing.phase === "error";

  // Any new computer on the account ends the wait, however it was added
  // (signed in there, or a code). The computers known on opening are not
  // new, nor are those listed in the first moments (the account's other
  // computers can arrive a little after this one's own list).
  const known = useRef<Set<string> | null>(null);
  const settledAt = useRef(0);
  const [arrived, setArrived] = useState<Machine | null>(null);
  useEffect(() => {
    if (!open) {
      known.current = null;
      setArrived(null);
    } else {
      settledAt.current = Date.now() + settleMs;
    }
  }, [open, settleMs]);
  useEffect(() => {
    if (!open || listed === undefined) return;
    const ids = computerIds(listed);
    if (known.current === null || Date.now() < settledAt.current) {
      known.current = new Set([...(known.current ?? []), ...ids]);
      return;
    }
    if (arrived) return;
    const fresh = listed.find((m) => !m.device && !m.current && !known.current!.has(m.id));
    if (fresh) setArrived(fresh);
  }, [open, listed, arrived]);
  const usedName = pairing.phase === "used" ? (machines.find((m) => m.id === pairing.code?.machine)?.name ?? null) : null;
  // Read the computers until a new one shows up, and, once a code was used,
  // until the computer that used it is listed (for its name).
  const waiting = open && signedIn && view !== "enter" && (pairing.phase === "used" ? !usedName : !arrived);
  useEffect(() => {
    if (!waiting) return;
    const timer = setInterval(() => void refresh().catch(() => {}), newComputerPollMs);
    return () => clearInterval(timer);
    // The hook's functions are new on every render.
  }, [waiting, newComputerPollMs]);

  const added = pairing.phase === "used" ? (usedName ?? arrived?.name ?? "the computer") : (arrived?.name ?? null);
  const forTeam = chosenTeam && pairing.phase === "used" ? teamName(chosenTeam) : null;

  const close = () => onOpenChange(false);
  const back = view === "pick" ? null : () => setView("pick");

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogPopup className="top-[9vh] w-[min(520px,calc(100vw-2rem))]" data-add-computer={view} data-add-machine={pairing.phase}>
        {view === "enter" ? (
          <EnterCode onBack={() => setView("pick")} onDone={close} initialCode={initialCode} initialRelay={initialRelay} open={open} />
        ) : (
          <>
            <div className="max-h-[80vh] overflow-y-auto px-6 pt-5 pb-5">
              <DialogTitle className="flex items-center gap-1 text-[15px] font-semibold">
                {back ? (
                  <Button size="icon-sm" variant="ghost" aria-label="Back" className="-ml-2" onClick={back} data-add-back="">
                    <ChevronLeftIcon />
                  </Button>
                ) : null}
                {TITLES[view]}
              </DialogTitle>
              {view === "pick" ? (
                <Pick
                  pairing={pairing}
                  noCode={noCode}
                  onPick={setView}
                />
              ) : view === "signin" ? (
                <SignInThere identity={session?.identity ?? null} done={added !== null} />
              ) : (
                <CodeMethod view={view} pairing={pairing} noCode={noCode} done={added !== null} />
              )}
            </div>
            <Footer
              view={view}
              pairing={pairing}
              signedIn={signedIn}
              noCode={noCode}
              added={added}
              forTeam={forTeam}
              thisName={thisComputerName(machines)}
              notice={notice?.text ?? null}
              onSignIn={() => void signIn().catch(() => {})}
              onEnter={() => setView("enter")}
              onByHand={() => setView("signin")}
              onClose={close}
              teamPicker={
                choices.teams.length > 0 && view === "pick" && !added ? (
                  <Select
                    aria-label="Add it for"
                    className="h-7 min-w-0 border-transparent bg-transparent text-xs text-muted-foreground shadow-none"
                    value={chosen ?? "me"}
                    options={[{ value: "me", label: "For Just me" }, ...choices.teams.map((t) => ({ value: t.id, label: `For ${teamName(t)}` }))]}
                    onValueChange={(v) => setPicked(v === "me" ? null : v)}
                  />
                ) : null
              }
            />
          </>
        )}
      </DialogPopup>
    </Dialog>
  );
}

/** The ids of the account's computers (not its other devices). */
function computerIds(list: readonly Machine[]): Set<string> {
  return new Set(list.filter((m) => !m.device).map((m) => m.id));
}

/** "This Mac" on a Mac, else "This computer". */
function thisComputerName(list: readonly Machine[]): string {
  return list.find((m) => m.current)?.os === "macos" ? "This Mac" : "This computer";
}

/* ---- The first page: the methods -------------------------------------------------- */

function Pick({ pairing, noCode, onPick }: { pairing: PairingHook; noCode: boolean; onPick: (v: AddView) => void }) {
  const code = pairing.code?.code ?? null;
  return (
    <>
      <DialogDescription className="mt-1 text-[13px] text-muted-foreground">Any Mac, Linux or Windows computer you own can run Spaces for you.</DialogDescription>
      <nav aria-label="Ways to add a computer" className="mt-3" data-add-methods="">
        <GroupedSection>
          {METHODS.map((m) => {
            const off = m.code && noCode;
            return (
              <button
                key={m.view}
                type="button"
                disabled={off}
                data-add-method={m.view}
                onClick={() => onPick(m.view)}
                className="flex min-h-11 w-full items-center gap-3 px-4 py-2.5 text-left text-[13px] outline-none hover:bg-accent/50 focus-visible:bg-accent disabled:opacity-45"
              >
                <span className="flex-1">{m.label}</span>
                <ChevronRightIcon className="size-3.5 text-muted-foreground/70" aria-hidden />
              </button>
            );
          })}
        </GroupedSection>
      </nav>
      {pairing.phase === "error" && pairing.error ? (
        <p className="mt-3 text-xs text-muted-foreground" data-add-code-error="">
          {pairing.error.unsupported ? "Your Cua relay can't make codes yet, so only Sign in there works." : `${pairing.error.message} `}
          {pairing.error.unsupported ? null : (
            <button type="button" className="text-brand outline-none hover:underline" onClick={pairing.renew} data-add-retry="">
              Try again
            </button>
          )}
        </p>
      ) : null}
      {noCode ? null : (
        <div className="mt-5" data-add-prompt="">
          <div className="flex items-center justify-between">
            <label htmlFor="add-agent-prompt" className="text-[13px] font-medium">
              Or paste this into an agent on that computer
            </label>
            <CopyButton text={code ? agentPrompt(code, pairing.code?.url ?? null) : ""} label="Copy prompt" disabled={!code || pairing.phase !== "ready"} />
          </div>
          <textarea
            id="add-agent-prompt"
            readOnly
            rows={2}
            value={code ? agentPrompt(code, pairing.code?.url ?? null) : "Making a code…"}
            style={{ fieldSizing: "content" } as CSSProperties}
            className="mt-1.5 block w-full resize-none rounded-lg bg-muted px-2.5 py-2 text-[12.5px] leading-relaxed text-foreground outline-none"
          />
        </div>
      )}
    </>
  );
}

/* ---- Sign in there ---------------------------------------------------------------- */

function SignInThere({ identity, done }: { identity: string | null; done: boolean }) {
  return (
    <div data-add-signin="">
      <DialogDescription className="mt-1 text-[13px] text-muted-foreground">
        On the other computer, open this link, install Cua Spaces and sign in{identity ? ` as ${identity}` : " with this account"}. It shows up here.
      </DialogDescription>
      <CopyLine text={INSTALL_URL} label="Copy link" dim={done} />
    </div>
  );
}

/* ---- The methods that use the one-time code --------------------------------------- */

function CodeMethod({ view, pairing, noCode, done }: { view: AddView; pairing: PairingHook; noCode: boolean; done: boolean }) {
  const [shell, setShell] = useState<Shell>("sh");
  const code = pairing.code;
  const dim = done || pairing.phase !== "ready";

  if (noCode) {
    return (
      <p className="mt-1 text-[13px] text-muted-foreground" data-add-no-code="">
        {pairing.error?.unsupported
          ? "Your Cua relay can't make codes yet. Use Sign in there instead."
          : pairing.error
            ? `${pairing.error.message} Use Sign in there instead.`
            : "This computer can't make a code. Use Sign in there instead."}
      </p>
    );
  }
  if (!code || pairing.phase === "minting" || pairing.phase === "idle") {
    return (
      <p className="mt-3 flex items-center gap-2 text-[13px] text-muted-foreground" data-pairing-minting="">
        <LoaderCircleIcon className="size-3.5 animate-spin" aria-hidden /> Making a code…
      </p>
    );
  }

  if (view === "link") {
    return (
      <div data-add-link="">
        <DialogDescription className="mt-1 text-[13px] text-muted-foreground">Open this link on the other computer. It shows the command that installs Cua Spaces and adds the computer here.</DialogDescription>
        {code.url ? <CopyLine text={code.url} label="Copy link" dim={dim} /> : null}
      </div>
    );
  }
  if (view === "qr") {
    return (
      <div data-add-qr="">
        <DialogDescription className="mt-1 text-[13px] text-muted-foreground">Scan it with the other computer's camera to open the same one-time link.</DialogDescription>
        {code.url ? (
          <div className={cn("mt-4 flex justify-center", dim && "opacity-45")}>
            <QrCode text={code.url} size={208} />
          </div>
        ) : null}
      </div>
    );
  }
  if (view === "command") {
    const command = shell === "sh" ? code.installSh : code.installPowershell;
    return (
      <div data-add-command="">
        <DialogDescription className="mt-1 text-[13px] text-muted-foreground">Run this on the other computer. It installs Cua Spaces and adds the computer here.</DialogDescription>
        <Segmented className="mt-3" aria-label="The other computer's system" value={shell} options={SHELLS} onValueChange={setShell} disabled={done} />
        {command ? <CopyLine text={command} label="Copy command" mono dim={dim} /> : null}
      </div>
    );
  }
  // Enter a code: for a computer that already has Cua Spaces.
  return (
    <div data-add-code="">
      <DialogDescription className="mt-1 text-[13px] text-muted-foreground">On the computer that has Cua Spaces, choose Add a computer, then Have a code? Enter it, and type:</DialogDescription>
      <div className={cn("mt-4 flex items-center justify-between gap-4", dim && "opacity-45")} data-pairing-code="">
        <code className="font-mono text-[28px] font-medium tracking-[0.08em] tabular-nums">{code.code}</code>
        <CopyButton text={code.code ?? ""} label="Copy code" disabled={dim} />
      </div>
      {pairing.phase === "ready" ? (
        <p className="mt-2 text-xs text-muted-foreground tabular-nums" data-pairing-expires="">
          Works once. {expiresInText(code.expiresAt, pairing.now)}.
        </p>
      ) : null}
    </div>
  );
}

/* ---- The footer ------------------------------------------------------------------- */

function Footer({
  view,
  pairing,
  signedIn,
  noCode,
  added,
  forTeam,
  thisName,
  notice,
  teamPicker,
  onSignIn,
  onEnter,
  onByHand,
  onClose,
}: {
  view: AddView;
  pairing: PairingHook;
  signedIn: boolean;
  noCode: boolean;
  added: string | null;
  forTeam: string | null;
  thisName: string;
  /** Why this computer can't make codes (not enrolled), in the host's words. */
  notice: string | null;
  teamPicker: ReactNode;
  onSignIn: () => void;
  onEnter: () => void;
  onByHand: () => void;
  onClose: () => void;
}) {
  const plans = usePlanLimitTelemetry();
  const plan = useRelayUsage()?.plan ?? null;
  const { openExternal } = useSession();
  const team = pairing.team;
  const limit = team?.phase === "failed" && team.upgradeUrl ? (team.kind ?? "other") : null;
  // Behind `upsell_cards`: a team owner out of seats or computers gets the
  // limit card (only an owner gets the checkout link) instead of Upgrade.
  const cards = useFlag("upsell_cards");
  const failedText = team?.phase === "failed" ? (team.error ?? "") : "";
  const cardReason: "seats" | "machines" | null = !limit
    ? null
    : limit === "seats" || /no free seats/.test(failedText)
      ? "seats"
      : limit === "machines" || /as many computers as it has seats/.test(failedText)
        ? "machines"
        : null;
  const asCard = cards && cardReason !== null;
  const shownLimit = useRef<string | null>(null);
  useEffect(() => {
    if (!limit) shownLimit.current = null;
    else if (shownLimit.current !== limit) {
      shownLimit.current = limit;
      plans.shown("app_team", { kind: limit, plan });
    }
  }, [limit, plans, plan]);

  const done = (
    <Button data-add-done="" variant={added ? "default" : "secondary"} onClick={onClose}>
      Done
    </Button>
  );

  if (added) {
    if (team?.phase === "adding") {
      return (
        <FooterBar>
          <Status kind="wait" data={{ "data-pairing-team": "adding" }}>
            {FINISHING_SETUP}
          </Status>
          {done}
        </FooterBar>
      );
    }
    return (
      <>
      {asCard ? <UpgradeCard reason={cardReason} className="mx-4 mb-3" /> : null}
      <FooterBar>
        <Status kind={team?.phase === "failed" ? "ended" : "ok"} data={{ "data-pairing-added": "", "data-pairing-team": team?.phase ?? "" }}>
          {team?.phase === "failed" ? `Added: ${added}. It isn't on ${forTeam ?? "your team"} yet. ${team.error ?? ""}`.trim() : forTeam ? `Added: ${added}, for ${forTeam}` : `Added: ${added}`}
        </Status>
        {team?.phase === "failed" && team.upgradeUrl && !asCard ? (
          <Button
            size="sm"
            variant="ghost"
            className="text-brand"
            data-pairing-upgrade=""
            onClick={() => {
              plans.opened("app_team", { kind: team.kind ?? "other", plan });
              void openExternal(plans.link(team.upgradeUrl!, "app_team")).catch(() => {});
            }}
          >
            Upgrade
          </Button>
        ) : null}
        {done}
      </FooterBar>
      </>
    );
  }

  if (view === "pick") {
    return (
      <FooterBar>
        <Button variant="secondary" onClick={onClose} data-add-cancel="">
          Cancel
        </Button>
        <span className="flex-1" />
        {teamPicker}
        {!signedIn ? (
          <span className="text-xs text-muted-foreground" data-add-signed-out="">
            <button type="button" className="text-brand outline-none hover:underline" onClick={onSignIn}>
              Sign in
            </button>{" "}
            to make a code.
          </span>
        ) : noCode && pairing.phase !== "error" ? (
          <span className="text-xs text-muted-foreground" data-add-no-code-note="" title={notice ?? undefined}>
            {thisName} can't make codes yet.
          </span>
        ) : null}
        <span className="text-xs text-muted-foreground">
          Have a code?{" "}
          <button type="button" className="text-brand outline-none hover:underline" onClick={onEnter} data-add-enter="">
            Enter it
          </button>
        </span>
      </FooterBar>
    );
  }

  const usesCode = view !== "signin" && !noCode;
  const ended = usesCode && (pairing.phase === "revoked" || pairing.phase === "expired") ? pairing.phase : null;
  return (
    <FooterBar>
      {ended ? (
        <Status kind="ended" data={{ "data-pairing-ended": ended }}>
          {ended === "revoked" ? "You revoked this code." : "This code expired."}
          <Button size="sm" variant="ghost" className="-ml-1 text-brand" onClick={pairing.renew} data-pairing-renew="">
            New code
          </Button>
        </Status>
      ) : (
        <Status kind="wait" data={{ "data-pairing-waiting": "" }}>
          {view === "signin" ? "Waiting for a new computer…" : "Waiting for the computer…"}
        </Status>
      )}
      {usesCode && pairing.code && pairing.phase === "ready" ? (
        <Menu
          label={<span data-pairing-menu-expires="">Works once. {expiresInText(pairing.code.expiresAt, pairing.now)}.</span>}
          trigger={
            <Button size="icon-sm" variant="ghost" aria-label="More" data-add-more="">
              <MoreHorizontalIcon />
            </Button>
          }
          items={[
            { label: "New code", onSelect: pairing.renew },
            { label: "Revoke", onSelect: () => void pairing.revoke().catch(() => {}) },
            { label: "Set it up by hand", onSelect: onByHand },
          ]}
        />
      ) : null}
      {done}
    </FooterBar>
  );
}

function FooterBar({ children }: { children: ReactNode }) {
  return <div /* design-ok: full-width dialog footer */ className="flex min-h-[57px] items-center gap-2 border-t px-6 py-3">{children}</div>;
}

function Status({ kind, children, data }: { kind: "wait" | "ok" | "ended"; children: ReactNode; data?: Record<string, string> }) {
  return (
    <span className="flex min-w-0 flex-1 items-center gap-2 text-[13px]" role="status" data-add-status={kind} {...data}>
      {kind === "wait" ? <LoaderCircleIcon className="size-3.5 shrink-0 animate-spin text-muted-foreground" aria-hidden /> : null}
      {kind === "ok" ? <CheckIcon className="size-4 shrink-0 text-success" aria-hidden /> : null}
      <span className={cn("min-w-0", kind === "ok" ? "text-foreground" : "text-muted-foreground")}>{children}</span>
    </span>
  );
}

/* ---- Enter a code made on another computer ------------------------------------------ */

function EnterCode({
  open,
  onBack,
  onDone,
  initialCode,
  initialRelay,
}: {
  open: boolean;
  onBack: () => void;
  onDone: () => void;
  initialCode: string;
  initialRelay: string | null;
}) {
  const { refresh } = useMachines();
  const joining = useJoinWithCode({ onJoined: () => void refresh().catch(() => {}) });
  const [text, setText] = useState(initialCode);
  const { reset } = joining;
  useEffect(() => {
    if (open) {
      setText(initialCode ? displayPairingCode(initialCode) : "");
      reset();
    }
  }, [open, initialCode, reset]);

  const busy = joining.phase === "joining";
  const joined = joining.phase === "joined";
  const ready = normalizePairingCode(text) !== null;
  const relay = initialRelay ?? pairingLinkRelay(text);
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!busy && !joined && ready) void joining.join(text, relay);
  };

  return (
    <form onSubmit={submit} data-join-with-code={joining.phase}>
      <div className="px-6 pt-5 pb-5">
        <DialogTitle className="flex items-center gap-1 text-[15px] font-semibold">
          <Button size="icon-sm" variant="ghost" aria-label="Back" className="-ml-2" onClick={onBack} disabled={busy} data-add-back="">
            <ChevronLeftIcon />
          </Button>
          Enter a code
        </DialogTitle>
        <DialogDescription className="mt-1 text-[13px] text-muted-foreground">
          From Add a computer on your other device. This computer joins that account. Only use a code you made or got from someone you trust.
        </DialogDescription>
        {relay ? (
          <p className="mt-2 text-xs text-muted-foreground" data-join-relay="">
            This code uses its own relay: <span className="text-foreground">{relay}</span>
          </p>
        ) : null}
        <Input
          autoFocus
          aria-label="Code"
          placeholder="XXXX-XXXX-XXXX"
          className="mt-4 font-mono tracking-wider"
          value={text}
          disabled={busy || joined}
          spellCheck={false}
          autoCapitalize="characters"
          autoComplete="off"
          onChange={(e) => setText(e.target.value)}
        />
        {joining.error ? (
          <p role="alert" className="mt-2 text-xs text-destructive" data-join-error="">
            {joining.error}
          </p>
        ) : null}
      </div>
      <FooterBar>
        {joined ? (
          <>
            <Status kind="ok" data={{ "data-join-joined": "" }}>
              Added this computer
            </Status>
            <Button type="button" onClick={onDone}>
              Done
            </Button>
          </>
        ) : busy ? (
          <>
            <Status kind="wait" data={{ "data-join-busy": "" }}>
              Setting this computer up…
            </Status>
            <Button type="button" variant="secondary" disabled>
              Cancel
            </Button>
          </>
        ) : (
          <>
            <Button type="button" variant="secondary" onClick={onDone}>
              Cancel
            </Button>
            <span className="flex-1" />
            <Button type="submit" disabled={!ready} data-join-submit="">
              Add this computer
            </Button>
          </>
        )}
      </FooterBar>
    </form>
  );
}

/* ---- Small pieces ----------------------------------------------------------------- */

/** A read-only line to copy: a link or a command. */
function CopyLine({ text, label, mono, dim }: { text: string; label: string; mono?: boolean; dim?: boolean }) {
  return (
    <div className={cn("mt-3 flex items-center gap-1 rounded-lg bg-muted pl-3", dim && "opacity-45")} data-copy-line="">
      <code className={cn("min-w-0 flex-1 py-2", mono ? "font-mono text-xs" : "truncate font-sans text-[13px]")} title={text}>
        {mono ? <Words text={text} /> : text}
      </code>
      <CopyButton text={text} label={label} disabled={dim} className="mr-1" />
    </div>
  );
}

/** A command, wrapped only between its words (never inside `--pair`). */
function Words({ text }: { text: string }) {
  const words = text.split(" ");
  return (
    <>
      {words.map((w, i) => (
        // The space stays outside the word, so the line can break there.
        <Fragment key={i}>
          <span className="whitespace-nowrap">{w}</span>
          {i < words.length - 1 ? " " : null}
        </Fragment>
      ))}
    </>
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
    <svg role="img" aria-label="QR code of the link" data-pairing-qr="" width={size} height={size} viewBox={`0 0 ${qr.size} ${qr.size}`} shapeRendering="crispEdges" className="rounded-lg bg-white">
      <path d={d} fill="#000" />
    </svg>
  );
}
