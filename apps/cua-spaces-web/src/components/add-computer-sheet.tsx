// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { CheckIcon, ChevronLeftIcon, ChevronRightIcon, LoaderCircleIcon, MoreHorizontalIcon } from "lucide-react";
import { Fragment, useEffect, useRef, useState, type CSSProperties, type FormEvent, type ReactNode } from "react";
import { Trans } from "react-i18next";
import { encode } from "uqr";

import {
  displayPairingCode,
  normalizePairingCode,
  pairingLinkRelay,
  teamName,
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
import { t as tr, useT } from "@/i18n";
import { cn } from "@/lib/utils";
import { THIS_MACHINE } from "@/bridge/host-os";

/** Where Cua Spaces is downloaded; the page picks the download for the computer that opens it. */
export const INSTALL_URL = "https://cua.ai/install";

/** How often the sheet reads the account's computers while it waits for a new one (ms). */
export const NEW_COMPUTER_POLL_MS = 3000;

/** How long after opening, computers that show up are the account's own, not new (ms). */
export const SETTLE_MS = 10_000;

/** The sheet's pages: the first page (the usual way, then the others), one page per other way, and entering a code made elsewhere. */
export type AddView = "pick" | "signin" | "link" | "qr" | "command" | "code" | "agent" | "enter";

type Shell = "sh" | "powershell";
const shells = (t: typeof tr): readonly { value: Shell; label: string }[] => [
  { value: "sh", label: t("computers.add.shell_sh") },
  { value: "powershell", label: t("computers.add.shell_powershell") },
];

/**
 * The ways to add a computer, in order. The first, signing in there, is how most people do it: the first page shows
 * it in full. The others are one tap away under "Other ways to add it", each needing a one-time code (`code`).
 */
export const METHODS: readonly { view: Exclude<AddView, "pick" | "enter">; code: boolean }[] = [
  { view: "signin", code: false },
  { view: "link", code: true },
  { view: "qr", code: true },
  { view: "command", code: true },
  { view: "code", code: true },
  { view: "agent", code: true },
];

/** The ways listed under "Other ways to add it". */
export const OTHER_METHODS = METHODS.slice(1);

/** The page titles; the methods' names are the same words. */
const titles = (t: typeof tr): Record<AddView, string> => ({
  pick: t("computers.add.title"),
  signin: t("computers.add.method_signin"),
  link: t("computers.add.method_link"),
  qr: t("computers.add.method_qr"),
  command: t("computers.add.method_command"),
  code: t("computers.add.method_code"),
  agent: t("computers.add.method_agent"),
  enter: t("computers.add.enter_title"),
});

/** "Expires in 29 min" (rounded up), "Expires in 40 s" in the last minute. */
function expiresText(t: typeof tr, expiresAtSecs: number, nowMs: number): string {
  const left = Math.max(0, expiresAtSecs * 1000 - nowMs);
  if (left < 60_000) return t("computers.add.expires_seconds", { count: Math.ceil(left / 1000) });
  return t("computers.add.expires_minutes", { count: Math.ceil(left / 60_000) });
}

/**
 * The prompt for an agent on the other computer: one line, one link. The
 * link's page tells the agent how to install with the code. `url` is the
 * link the core made (it keeps a non-default relay); without it, the plain
 * add link for the code.
 */
export function agentPrompt(code: string, url: string | null = null): string {
  return tr("computers.add.agent_prompt", { url: url ?? `https://cua.ai/add/${code}` });
}

/**
 * "Add a computer": a title, one line, and the usual way in full (install
 * Cua Spaces there and sign in, with the link to copy). The other ways
 * (Open a link, Scan a QR code, Run a command, Type a code there, Ask an
 * agent there) sit under "Other ways to add it", each opening only what it
 * needs. The footer waits for the computer and says
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
  const t = useT();
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
  const chosenTeam = chosen ? choices.all.find((c) => c.id === chosen) : undefined;

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

  const added = pairing.phase === "used" ? (usedName ?? arrived?.name ?? t("computers.add.the_computer")) : (arrived?.name ?? null);
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
                  <Button size="icon-sm" variant="ghost" aria-label={t("computers.add.back")} className="-ml-2" onClick={back} data-add-back="">
                    <ChevronLeftIcon />
                  </Button>
                ) : null}
                {titles(t)[view]}
              </DialogTitle>
              {view === "pick" ? (
                <Pick pairing={pairing} noCode={noCode} identity={session?.identity ?? null} done={added !== null} open={open} onPick={setView} />
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
              thisName={thisComputerName(t, machines)}
              notice={notice?.text ?? null}
              onSignIn={() => void signIn().catch(() => {})}
              onEnter={() => setView("enter")}
              onByHand={() => setView("signin")}
              onClose={close}
              teamPicker={
                choices.teams.length > 0 && view === "pick" && !added ? (
                  <Select
                    aria-label={t("computers.add.add_it_for")}
                    className="h-7 min-w-0 border-transparent bg-transparent text-xs text-muted-foreground shadow-none"
                    value={chosen ?? "me"}
                    options={[
                      { value: "me", label: t("computers.add.for_just_me") },
                      ...choices.teams.map((c) => ({ value: c.id, label: t("computers.add.for_team", { team: teamName(c) }) })),
                    ]}
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

/** What this computer is called, in the words of its own system ("This Mac", "This PC", "This machine"). */
function thisComputerName(t: typeof tr, list: readonly Machine[]): string {
  const os = list.find((m) => m.current)?.os;
  return os === "macos" || os === "windows" || os === "linux" ? THIS_MACHINE[os] : t("computers.add.this_machine");
}

/* ---- The first page: the methods -------------------------------------------------- */

function Pick({
  pairing,
  noCode,
  identity,
  done,
  open,
  onPick,
}: {
  pairing: PairingHook;
  noCode: boolean;
  identity: string | null;
  done: boolean;
  open: boolean;
  onPick: (v: AddView) => void;
}) {
  const t = useT();
  const names = titles(t);
  const [others, setOthers] = useState(false);
  useEffect(() => {
    if (!open) setOthers(false);
  }, [open]);
  return (
    <>
      <DialogDescription className="mt-1 text-[13px] text-muted-foreground">{t("computers.add.lede")}</DialogDescription>
      {/* The usual way, in full: nothing to pick first. */}
      <div className="mt-4" data-add-primary="" data-add-signin="">
        <p className="text-[13px] font-medium">{t("computers.add.primary_title")}</p>
        <p className="mt-1 text-[13px] text-muted-foreground">
          {identity ? t("computers.add.signin_as", { identity }) : t("computers.add.signin_this_account")}
        </p>
        <CopyLine text={INSTALL_URL} label={t("computers.add.copy_link")} dim={done} />
      </div>
      {pairing.phase === "error" && pairing.error ? (
        <p className="mt-3 text-xs text-muted-foreground" data-add-code-error="">
          {pairing.error.unsupported ? t("computers.add.codes_unsupported") : `${pairing.error.message} `}
          {pairing.error.unsupported ? null : (
            <button type="button" className="text-brand outline-none hover:underline" onClick={pairing.renew} data-add-retry="">
              {t("computers.add.try_again")}
            </button>
          )}
        </p>
      ) : null}
      {noCode ? null : (
        <div className="mt-5">
          <button
            type="button"
            aria-expanded={others}
            aria-controls="add-other-ways"
            onClick={() => setOthers((o) => !o)}
            className="flex items-center gap-1 text-[13px] font-medium text-muted-foreground outline-none hover:text-foreground focus-visible:text-foreground"
            data-add-other-toggle=""
          >
            <ChevronRightIcon className={cn("size-3.5 transition-transform motion-reduce:transition-none", others && "rotate-90")} aria-hidden />
            {t("computers.add.other_ways")}
          </button>
          {others ? (
            <nav id="add-other-ways" aria-label={t("computers.add.methods_label")} className="mt-2" data-add-methods="">
              <GroupedSection>
                {OTHER_METHODS.map((m) => (
                  <button
                    key={m.view}
                    type="button"
                    data-add-method={m.view}
                    onClick={() => onPick(m.view)}
                    className="flex min-h-11 w-full items-center gap-3 px-4 py-2.5 text-left text-[13px] outline-none hover:bg-accent/50 focus-visible:bg-accent"
                  >
                    <span className="flex-1">{names[m.view]}</span>
                    <ChevronRightIcon className="size-3.5 text-muted-foreground/70" aria-hidden />
                  </button>
                ))}
              </GroupedSection>
            </nav>
          ) : null}
        </div>
      )}
    </>
  );
}

/* ---- Sign in there ---------------------------------------------------------------- */

function SignInThere({ identity, done }: { identity: string | null; done: boolean }) {
  const t = useT();
  return (
    <div data-add-signin="">
      <DialogDescription className="mt-1 text-[13px] text-muted-foreground">
        {identity ? t("computers.add.signin_as", { identity }) : t("computers.add.signin_this_account")}
      </DialogDescription>
      <CopyLine text={INSTALL_URL} label={t("computers.add.copy_link")} dim={done} />
    </div>
  );
}

/* ---- The methods that use the one-time code --------------------------------------- */

function CodeMethod({ view, pairing, noCode, done }: { view: AddView; pairing: PairingHook; noCode: boolean; done: boolean }) {
  const t = useT();
  const [shell, setShell] = useState<Shell>("sh");
  const code = pairing.code;
  const dim = done || pairing.phase !== "ready";

  if (noCode) {
    return (
      <p className="mt-1 text-[13px] text-muted-foreground" data-add-no-code="">
        {pairing.error?.unsupported
          ? t("computers.add.codes_unsupported")
          : pairing.error
            ? t("computers.add.code_error_use_signin", { message: pairing.error.message })
            : t("computers.add.cannot_make_code")}
      </p>
    );
  }
  if (!code || pairing.phase === "minting" || pairing.phase === "idle") {
    return (
      <p className="mt-3 flex items-center gap-2 text-[13px] text-muted-foreground" data-pairing-minting="">
        <LoaderCircleIcon className="size-3.5 animate-spin" aria-hidden /> {t("computers.add.making_code")}
      </p>
    );
  }

  if (view === "link") {
    return (
      <div data-add-link="">
        <DialogDescription className="mt-1 text-[13px] text-muted-foreground">{t("computers.add.link_help")}</DialogDescription>
        {code.url ? <CopyLine text={code.url} label={t("computers.add.copy_link")} dim={dim} /> : null}
      </div>
    );
  }
  if (view === "qr") {
    return (
      <div data-add-qr="">
        <DialogDescription className="mt-1 text-[13px] text-muted-foreground">{t("computers.add.qr_help")}</DialogDescription>
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
        <DialogDescription className="mt-1 text-[13px] text-muted-foreground">{t("computers.add.command_help")}</DialogDescription>
        <Segmented className="mt-3" aria-label={t("computers.add.other_system")} value={shell} options={shells(t)} onValueChange={setShell} disabled={done} />
        {command ? <CopyLine text={command} label={t("computers.add.copy_command")} mono dim={dim} /> : null}
      </div>
    );
  }
  if (view === "agent") {
    const prompt = agentPrompt(code.code ?? "", code.url ?? null);
    return (
      <div data-add-prompt="">
        <DialogDescription className="mt-1 text-[13px] text-muted-foreground">{t("computers.add.agent_help")}</DialogDescription>
        <div className="mt-3 flex justify-end">
          <CopyButton text={prompt} label={t("computers.add.copy_prompt")} disabled={dim} />
        </div>
        <textarea
          id="add-agent-prompt"
          aria-label={t("computers.add.method_agent")}
          readOnly
          rows={2}
          value={prompt}
          style={{ fieldSizing: "content" } as CSSProperties}
          className={cn(
            "mt-1.5 block w-full resize-none rounded-lg bg-muted px-2.5 py-2 text-[12.5px] leading-relaxed text-foreground outline-none",
            dim && "opacity-45",
          )}
        />
      </div>
    );
  }
  // Type a code there: for a computer that already has Cua Spaces.
  return (
    <div data-add-code="">
      <DialogDescription className="mt-1 text-[13px] text-muted-foreground">{t("computers.add.code_help")}</DialogDescription>
      <div className={cn("mt-4 flex items-center justify-between gap-4", dim && "opacity-45")} data-pairing-code="">
        <code className="font-mono text-[28px] font-medium tracking-[0.08em] tabular-nums">{code.code}</code>
        <CopyButton text={code.code ?? ""} label={t("computers.add.copy_code")} disabled={dim} />
      </div>
      {pairing.phase === "ready" ? (
        <p className="mt-2 text-xs text-muted-foreground tabular-nums" data-pairing-expires="">
          {t("computers.add.works_once", { expires: expiresText(t, code.expiresAt, pairing.now) })}
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
  const t = useT();
  const plans = usePlanLimitTelemetry();
  const plan = useRelayUsage()?.plan ?? null;
  const { openExternal } = useSession();
  const team = pairing.team;
  const limit = team?.phase === "failed" && team.upgradeUrl ? (team.kind ?? "other") : null;
  // A team owner out of seats or computers gets the limit card (only an owner
  // gets the checkout link) instead of Upgrade.
  const failedText = team?.phase === "failed" ? (team.error ?? "") : "";
  const cardReason: "seats" | "machines" | null = !limit
    ? null
    : limit === "seats" || /no free seats/.test(failedText)
      ? "seats"
      : limit === "machines" || /as many machines as it has seats/.test(failedText)
        ? "machines"
        : null;
  const asCard = cardReason !== null;
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
      {t("computers.add.done")}
    </Button>
  );

  if (added) {
    if (team?.phase === "adding") {
      return (
        <FooterBar>
          <Status kind="wait" data={{ "data-pairing-team": "adding" }}>
            {t("computers.add.finishing_setup")}
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
            {team?.phase === "failed"
              ? t("computers.add.added_not_on_team", { name: added, team: forTeam ?? t("computers.add.your_team"), error: team.error ?? "" }).trim()
              : forTeam
                ? t("computers.add.added_for_team", { name: added, team: forTeam })
                : t("computers.add.added", { name: added })}
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
              {t("computers.add.upgrade")}
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
          {t("computers.add.cancel")}
        </Button>
        <span className="flex-1" />
        {teamPicker}
        {!signedIn ? (
          <span className="text-xs text-muted-foreground" data-add-signed-out="">
            <Trans
              i18nKey="computers.add.sign_in_to_make_code"
              ns="computers"
              components={{ action: <button type="button" className="text-brand outline-none hover:underline" onClick={onSignIn} /> }}
            />
          </span>
        ) : noCode && pairing.phase !== "error" ? (
          <span className="text-xs text-muted-foreground" data-add-no-code-note="" title={notice ?? undefined}>
            {t("computers.add.cannot_make_codes_yet", { name: thisName })}
          </span>
        ) : null}
        <span className="text-xs text-muted-foreground">
          <Trans
            i18nKey="computers.add.have_code"
            ns="computers"
            components={{ action: <button type="button" className="text-brand outline-none hover:underline" onClick={onEnter} data-add-enter="" /> }}
          />
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
          {ended === "revoked" ? t("computers.add.code_revoked") : t("computers.add.code_expired")}
          <Button size="sm" variant="ghost" className="-ml-1 text-brand" onClick={pairing.renew} data-pairing-renew="">
            {t("computers.add.new_code")}
          </Button>
        </Status>
      ) : (
        <Status kind="wait" data={{ "data-pairing-waiting": "" }}>
          {view === "signin" ? t("computers.add.waiting_new") : t("computers.add.waiting")}
        </Status>
      )}
      {usesCode && pairing.code && pairing.phase === "ready" ? (
        <Menu
          label={<span data-pairing-menu-expires="">{t("computers.add.works_once", { expires: expiresText(t, pairing.code.expiresAt, pairing.now) })}</span>}
          trigger={
            <Button size="icon-sm" variant="ghost" aria-label={t("computers.add.more")} data-add-more="">
              <MoreHorizontalIcon />
            </Button>
          }
          items={[
            { label: t("computers.add.new_code"), onSelect: pairing.renew },
            { label: t("computers.add.revoke"), onSelect: () => void pairing.revoke().catch(() => {}) },
            { label: t("computers.add.set_up_by_hand"), onSelect: onByHand },
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
  const t = useT();
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
          <Button size="icon-sm" variant="ghost" aria-label={t("computers.add.back")} className="-ml-2" onClick={onBack} disabled={busy} data-add-back="">
            <ChevronLeftIcon />
          </Button>
          {t("computers.add.method_code")}
        </DialogTitle>
        <DialogDescription className="mt-1 text-[13px] text-muted-foreground">{t("computers.add.enter_trust")}</DialogDescription>
        {relay ? (
          <p className="mt-2 text-xs text-muted-foreground" data-join-relay="">
            <Trans i18nKey="computers.add.own_relay" ns="computers" values={{ relay }} components={{ host: <span className="text-foreground" /> }} />
          </p>
        ) : null}
        <Input
          autoFocus
          aria-label={t("computers.add.code_label")}
          placeholder={t("computers.add.code_placeholder")}
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
              {t("computers.add.joined")}
            </Status>
            <Button type="button" onClick={onDone}>
              {t("computers.add.done")}
            </Button>
          </>
        ) : busy ? (
          <>
            <Status kind="wait" data={{ "data-join-busy": "" }}>
              {t("computers.add.joining")}
            </Status>
            <Button type="button" variant="secondary" disabled>
              {t("computers.add.cancel")}
            </Button>
          </>
        ) : (
          <>
            <Button type="button" variant="secondary" onClick={onDone}>
              {t("computers.add.cancel")}
            </Button>
            <span className="flex-1" />
            <Button type="submit" disabled={!ready} data-join-submit="">
              {t("computers.add.add_this_computer")}
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
  const t = useT();
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
      aria-label={t("computers.add.qr_alt")}
      data-pairing-qr=""
      width={size}
      height={size}
      viewBox={`0 0 ${qr.size} ${qr.size}`}
      shapeRendering="crispEdges"
      className="rounded-lg bg-white"
    >
      <path d={d} fill="#000" />
    </svg>
  );
}
