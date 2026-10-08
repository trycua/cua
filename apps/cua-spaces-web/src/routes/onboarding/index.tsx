// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { lazy, Suspense, useEffect, useState, type ReactNode } from "react";

import { useOnboarding, useOnboardingVolume, useSession, useSpaces, useThisMachine, type OnboardingHook, type OnboardingView, type SandboxImage } from "@/bridge";
import { CuaLogo } from "@/components/cua-logo";
import { useAgentsStep, type AgentsStep } from "@/components/volume/use-agents-step";
import { OsIcon } from "@/components/os-icon";
import { ModeCard } from "@/components/onboarding-mode";
import { pressPrimaryOnReturn } from "@/components/onboarding-keys";
import { SetUpLater } from "@/components/onboarding-skip";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Switch } from "@/components/ui/switch";
import { toast } from "@/components/ui/toast";
import { cn } from "@/lib/utils";

export const Route = createFileRoute("/onboarding/")({ component: OnboardingPage });

// The AI agents and Cua Volume cards load when their page shows.
const AgentsCard = lazy(() => import("@/components/volume/agents-step").then((m) => ({ default: m.AgentsCard })));
const DriveCardView = lazy(() => import("@/components/volume/drive-card").then((m) => ({ default: m.DriveCardView })));

/** Sign in is optional: everything local works without an account. */
const NO_ACCOUNT_NEEDED = "No account needed to start.";

/**
 * First run, laid out like the SwiftUI app's (`OnboardingView.swift`):
 * Welcome centred, every other page in two columns, Back bottom-left, the
 * page dots centred, Skip and the primary action bottom-right. Pages, copy,
 * which buttons show and the answers come from the app core through
 * `useOnboarding`; this file only places them.
 */
function OnboardingPage() {
  const flow = useOnboarding();
  const navigate = useNavigate();
  const { view } = flow;
  useOnboardingVolume(flow);
  const agents = useAgentsStep(view?.step === "agents");

  const leave = () => {
    flow.skip();
    void navigate({ to: "/spaces" });
  };
  const finish = async () => {
    try {
      await flow.finish();
      await navigate({ to: "/spaces" });
    } catch (e) {
      toast("Couldn't finish setup", { description: e instanceof Error ? e.message : String(e) });
    }
  };

  // A new page: focus moves to its heading, so a screen reader reads it
  // and Return goes to the page (Welcome focuses Get started itself).
  const step = view?.step;
  useEffect(() => {
    if (step && step !== "welcome") document.querySelector<HTMLElement>("[data-onboarding-heading]")?.focus();
  }, [step]);

  // Hold the frame while the session loads, so the page never flashes empty.
  if (!view) return <div className="h-full" aria-busy="true" />;

  return (
    <div className="relative flex h-full flex-col" data-step={view.step} onKeyDown={pressPrimaryOnReturn}>
      {view.step !== "done" ? <SetUpLater onClick={leave} /> : null}
      <div className="flex min-h-0 flex-1 flex-col items-center justify-center overflow-y-auto px-10 py-6">
        {view.step === "welcome" ? <Welcome flow={flow} view={view} /> : <Columns flow={flow} view={view} agents={agents} />}
      </div>
      <Footer flow={flow} view={view} agents={agents} onFinish={() => void finish()} />
    </div>
  );
}

/* ---- Welcome ----------------------------------------------------------------- */

function Welcome({ flow, view }: { flow: OnboardingHook; view: OnboardingView }) {
  const { openExternal } = useSession();
  const usage = view.usage;
  // Welcome is on screen: the core counts the run now when this machine
  // already showed the usage notice (else when Welcome is left).
  const settingRead = Boolean(flow.state?.telemetry);
  useEffect(() => {
    if (settingRead) flow.send({ type: "welcome-shown" });
    // Once per showing, after the setting was read.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [settingRead]);
  return (
    <div className="flex w-full max-w-[520px] flex-col items-center text-center">
      {view.showMark ? <CuaLogo className="mb-6 size-14" /> : null}
      <h1 className="text-[26px] font-semibold tracking-[-0.015em]">{view.title}</h1>
      <p className="mt-2 text-[14px] text-muted-foreground">{view.lede}</p>
      <Button size="lg" className="mt-8 min-w-36" autoFocus onClick={() => flow.send({ type: "start" })}>
        {view.primaryLabel}
      </Button>
      <div className="mt-10 flex flex-col items-center gap-2.5">
        {usage ? (
          <label className="flex items-center gap-2.5 text-[13px]" title={usage.help ?? undefined}>
            <Switch
              checked={usage.on}
              disabled={!usage.enabled}
              onCheckedChange={(on) => flow.send({ type: "usage-data-toggled", on })}
              data-testid="onboarding-share-usage"
            />
            {usage.label}
            <span className="text-muted-foreground">{usage.on ? "On" : "Off"}</span>
          </label>
        ) : null}
        {usage?.help ? <p className="text-xs text-muted-foreground">{usage.help}</p> : null}
        {view.notice ? (
          <p className="text-xs text-pretty text-muted-foreground">
            {view.notice}{" "}
            {view.noticeLinkLabel && view.noticeLinkUrl ? (
              <button
                type="button"
                className="text-foreground underline-offset-2 hover:underline"
                onClick={() => void openExternal(view.noticeLinkUrl!)}
              >
                {view.noticeLinkLabel}
              </button>
            ) : null}
          </p>
        ) : null}
      </div>
    </div>
  );
}

/* ---- Two-column pages --------------------------------------------------------- */

function Columns({ flow, view, agents }: { flow: OnboardingHook; view: OnboardingView; agents: AgentsStep }) {
  const { data: session } = useSession();
  // Host setup's form, opened from the host choice on This machine.
  const host = useThisMachine(session?.identity ?? null);
  const settingUpHost = view.step === "mode" && host.formView !== null;
  // Leaving the page (Back) leaves the form too.
  const formOpen = host.form !== null;
  useEffect(() => {
    if (view.step !== "mode" && formOpen) host.closeForm();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [view.step, formOpen]);
  return (
    <div className="w-full max-w-[860px]">
      <div className="grid grid-cols-[minmax(0,1fr)_minmax(0,1.1fr)] items-center gap-14">
        <div>
          <h1 tabIndex={-1} data-onboarding-heading="" className="text-[22px] font-semibold tracking-[-0.01em] outline-none">
            {view.title}
          </h1>
          <p className="mt-2 text-[14px] text-muted-foreground">{view.lede}</p>
          {view.step === "signin" ? <SignInAside flow={flow} /> : null}
        </div>
        <div>
          {view.step === "signin" ? <SignInCard flow={flow} /> : null}
          {view.step === "agents" ? (
            <Card>
              <Suspense fallback={<p className="text-[13px] text-muted-foreground">{agents.copy.agentsLooking}</p>}>
                <AgentsCard step={agents} />
              </Suspense>
            </Card>
          ) : null}
          {view.step === "drive" && view.drive ? (
            <Card>
              <Suspense fallback={null}>
              <DriveCardView
                card={view.drive}
                act={{
                  toggle: (on) => flow.send({ type: "drive-toggled", on }),
                  chooseStorage: (choice) => flow.send({ type: "storage-chosen", choice }),
                  storage: (action) => flow.send({ type: "drive-storage", action }),
                }}
              />
              </Suspense>
            </Card>
          ) : null}
          {view.step === "mode" ? <ModeCard flow={flow} view={view} host={host} /> : null}
          {view.step === "done" ? <DoneCard view={view} flow={flow} /> : null}
        </div>
      </div>
      {/* Only hosts need Screen Recording and Accessibility (the core's host setup). */}
      {settingUpHost && flow.permissions.length > 0 ? <Permissions flow={flow} /> : null}
      {view.step === "done" ? <FirstSpace images={flow.images} /> : null}
    </div>
  );
}

function Card({ children, className, ...rest }: { children: ReactNode; className?: string } & Record<`data-${string}`, string>) {
  return (
    <div className={cn("rounded-xl border bg-card px-5 py-4 shadow-xs", className)} {...rest}>
      {children}
    </div>
  );
}

function SignInAside({ flow }: { flow: OnboardingHook }) {
  const { openExternal } = useSession();
  return (
    <>
      <p className="mt-4 text-[13px]">{NO_ACCOUNT_NEEDED}</p>
      <p className="mt-3 text-xs text-muted-foreground">
        {flow.copy.teams} ·{" "}
        <button type="button" className="text-foreground underline-offset-2 hover:underline" onClick={() => void openExternal(flow.copy.teamsUrl)}>
          {flow.copy.teamsLink}
        </button>
      </p>
    </>
  );
}

function SignInCard({ flow }: { flow: OnboardingHook }) {
  const { data: session, signOut, cancelSignIn, openExternal } = useSession();
  const phase = session?.signIn.kind ?? "idle";
  if (flow.state?.identity) {
    return (
      <Card className="flex items-center justify-between gap-4">
        <span className="min-w-0 truncate text-[13px]">{flow.signInText}</span>
        {session?.fleet.authMode === "user" ? (
          <Button variant="ghost" size="sm" onClick={() => void signOut()}>
            Sign out
          </Button>
        ) : null}
      </Card>
    );
  }
  if (phase === "starting" || phase === "waiting") {
    // The code to confirm, the page to open again if the tab was closed,
    // and a way out (it also times out by itself).
    const url = session?.signInUrl;
    return (
      <Card data-signin="waiting">
        <p className="text-[13px] text-muted-foreground" role="status">
          {phase === "starting" ? flow.copy.signInWaiting : flow.signInText}
        </p>
        <div className="mt-3 flex items-center gap-3 text-xs">
          {url ? (
            <button
              type="button"
              className="text-foreground underline-offset-2 hover:underline"
              onClick={() => void openExternal(url)}
              data-signin-reopen=""
            >
              Open the browser again
            </button>
          ) : null}
          <button
            type="button"
            className="text-muted-foreground underline-offset-2 hover:text-foreground hover:underline"
            onClick={() => cancelSignIn()}
            data-signin-cancel=""
          >
            Cancel
          </button>
        </div>
      </Card>
    );
  }
  if (phase === "failed") {
    return (
      <Card data-signin="failed">
        <p className="text-[13px] text-destructive" role="alert">
          {flow.signInText}
        </p>
      </Card>
    );
  }
  return null;
}

/** Explainers only: the grant itself is macOS's, in System Settings. */
function Permissions({ flow }: { flow: OnboardingHook }) {
  const { openExternal } = useSession();
  return (
    <section className="mt-10" aria-labelledby="onboarding-permissions">
      <h2 id="onboarding-permissions" className="mb-1 text-[13px] font-semibold">
        {flow.copy.permissionsTitle}
      </h2>
      <p className="mb-3 text-xs text-muted-foreground">macOS asks for each one itself. Cua Spaces can't turn them on for you.</p>
      <Card className="divide-y p-0">
        {flow.permissions.map((p) => {
          const url = p.settingsUrl?.startsWith("x-apple.systempreferences:") ? p.settingsUrl : null;
          return (
            <div key={p.id} className="flex items-center justify-between gap-6 px-4 py-3">
              <div className="min-w-0">
                <div className="text-[13px]">{p.title}</div>
                {p.help ? <div className="mt-0.5 text-xs text-muted-foreground">{p.help}</div> : null}
              </div>
              {url ? (
                <Button variant="outline" size="sm" onClick={() => void openExternal(url)}>
                  {flow.copy.openSettings}
                </Button>
              ) : null}
            </div>
          );
        })}
      </Card>
    </section>
  );
}

function DoneCard({ view, flow }: { view: OnboardingView; flow: OnboardingHook }) {
  // "Launch at login", where the host applies it (the SwiftUI app's Done has it too).
  const login = flow.launchAtLogin;
  return (
    <Card>
      <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-6 gap-y-1.5 text-[13px]">
        {view.summary.map((f) => (
          <div key={f.label} className="contents">
            <dt className="text-muted-foreground">{f.label}</dt>
            <dd className="truncate">{f.value}</dd>
          </div>
        ))}
      </dl>
      {login ? (
        <div className="mt-4 flex flex-col gap-1">
          <label className="flex items-center gap-2.5 text-[13px]">
            <Checkbox
              checked={login.checked}
              onCheckedChange={(on) => flow.send({ type: "launch-at-login-toggled", on: Boolean(on) })}
              data-onboarding-launch-at-login=""
            />
            {login.label}
          </label>
          {login.note ? <p className="pl-[26px] text-xs text-muted-foreground">{login.note}</p> : null}
        </div>
      ) : null}
    </Card>
  );
}

function FirstSpace({ images }: { images: SandboxImage[] }) {
  const { createSpace } = useSpaces();
  const [started, setStarted] = useState<SandboxImage | null>(null);
  const create = (image: SandboxImage) => {
    setStarted(image);
    createSpace({ image: image.ref, os: image.os }).catch((e: unknown) => {
      setStarted(null);
      toast(`Couldn't create ${image.name}`, { description: e instanceof Error ? e.message : String(e) });
    });
  };
  return (
    <section className="mt-10" aria-labelledby="onboarding-first-space">
      <h2 id="onboarding-first-space" className="mb-1 text-[13px] font-semibold">
        Create your first Space
      </h2>
      <p className="mb-3 text-xs text-muted-foreground">
        {started ? `Creating ${started.name}. It shows in Spaces when it's ready.` : "A separate desktop for an agent. You can also do this later."}
      </p>
      <ul className="grid grid-cols-3 gap-3">
        {images.map((image) => (
          <li key={image.ref}>
            <Card className="flex items-center gap-3 px-4 py-3">
              <OsIcon os={image.os} className="size-4 shrink-0 text-muted-foreground" />
              <div className="min-w-0 flex-1">
                <div className="truncate text-[13px] font-medium">{image.name}</div>
                <div className="truncate text-xs text-muted-foreground">{image.variant === "vm" ? "Virtual machine" : "Container"}</div>
              </div>
              <Button variant="outline" size="sm" disabled={started !== null} onClick={() => create(image)}>
                {started?.ref === image.ref ? "Creating…" : "Create"}
              </Button>
            </Card>
          </li>
        ))}
      </ul>
    </section>
  );
}

/* ---- Footer ---------------------------------------------------------------------- */

function Footer({ flow, view, agents, onFinish }: { flow: OnboardingHook; view: OnboardingView; agents: AgentsStep; onFinish: () => void }) {
  return (
    <div className="relative flex h-16 shrink-0 items-center px-6 pb-2">
      <div className="z-10">
        {view.canBack && view.step !== "welcome" ? (
          <Button variant="ghost" size="lg" onClick={() => flow.send({ type: "back" })}>
            {flow.copy.back}
          </Button>
        ) : null}
      </div>
      <Dots view={view} />
      <div className="z-10 ml-auto flex items-center gap-2">{view.step !== "welcome" ? <Trailing flow={flow} view={view} agents={agents} onFinish={onFinish} /> : null}</div>
    </div>
  );
}

function Dots({ view }: { view: OnboardingView }) {
  const current = view.dots.findIndex((d) => d.current);
  return (
    <ol className="absolute inset-x-0 flex justify-center gap-2" aria-label={`Step ${current + 1} of ${view.dots.length}`}>
      {view.dots.map((d) => (
        <li
          key={d.step}
          title={d.label}
          aria-current={d.current ? "step" : undefined}
          className={cn("size-1.5 rounded-full bg-foreground/20", d.current && "bg-foreground/70")}
        >
          <span className="sr-only">{d.label}</span>
        </li>
      ))}
    </ol>
  );
}

function Trailing({ flow, view, agents, onFinish }: { flow: OnboardingHook; view: OnboardingView; agents: AgentsStep; onFinish: () => void }) {
  const { data: session, signIn } = useSession();
  const skip = view.canSkip ? (
    <Button variant="ghost" size="lg" onClick={() => flow.send({ type: "signin-done" })}>
      {flow.copy.skip}
    </Button>
  ) : null;

  switch (view.step) {
    case "signin": {
      if (flow.state?.identity) {
        return (
          <Button size="lg" data-onboarding-primary="" onClick={() => flow.send({ type: "signin-done" })}>
            {view.primaryLabel}
          </Button>
        );
      }
      const phase = session?.signIn.kind ?? "idle";
      const waiting = phase === "starting" || phase === "waiting";
      return (
        <>
          {skip}
          <Button
            size="lg"
            data-onboarding-primary=""
            disabled={!session || waiting}
            onClick={() => signIn().catch(() => {}) /* the failure shows on the page */}
          >
            {waiting ? flow.copy.signInWaiting : phase === "failed" ? flow.copy.tryAgain : flow.copy.signIn}
          </Button>
        </>
      );
    }
    case "agents":
      return (
        <>
          {agents.summaries ? null : (
            <Button variant="ghost" size="lg" disabled={agents.busy} onClick={() => flow.send({ type: "agents-done", configured: [] })}>
              {flow.copy.skip}
            </Button>
          )}
          {agents.summaries ? (
            <Button size="lg" data-onboarding-primary="" onClick={() => flow.send({ type: "agents-done", configured: agents.configured })}>
              {view.primaryLabel}
            </Button>
          ) : (
            <Button size="lg" data-onboarding-primary="" disabled={!agents.canSetUp} onClick={() => void agents.setUp()} data-agents-set-up="">
              {agents.busy ? agents.copy.agentsSettingUp : agents.copy.agentsSetUp}
            </Button>
          )}
        </>
      );
    case "drive":
      return (
        <Button
          size="lg"
          disabled={!view.drive || view.drive.busy || !view.drive.canContinue}
          onClick={() => flow.send({ type: "drive-continue" })}
          data-drive-continue=""
          data-onboarding-primary=""
        >
          {view.primaryLabel}
        </Button>
      );
    case "mode":
      // The choices are the buttons.
      return null;
    case "done":
      return (
        <Button size="lg" data-onboarding-primary="" onClick={onFinish}>
          {view.primaryLabel}
        </Button>
      );
    default:
      // Welcome has its own button; the native-only pages never show here.
      return null;
  }
}
