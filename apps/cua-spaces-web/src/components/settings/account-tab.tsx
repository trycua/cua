// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import {
  useRelayUsage,
  useRelayUsageView,
  useSession,
  useSettings,
  useTrackPlanClick,
  type RelayUsage,
  type SettingsRow as CoreRow,
  type SettingsSection,
} from "@/bridge";
import { SettingsGroup, SettingsRow } from "@/components/settings-group";
import { CoreSettingsRow } from "@/components/settings/core-section";
import { Button } from "@/components/ui/button";
import { toastError } from "@/components/ui/toast";
import { useT } from "@/i18n";

const fail = toastError;

/** "Free plan" as "Free", for a row that already says Plan. */
const planName = (label: string) => label.replace(/ plan$/i, "");

/** 6.5, 20 and 6 hours: one decimal at most, none when whole. */
const hours = (n: number) => String(Math.round(n * 10) / 10);

/** The team the core names ("Acme Robotics", "Owner · Pro plan"), as "Acme Robotics, Owner". */
function teamLine(teams: SettingsSection | undefined, say: ReturnType<typeof useT>): string | null {
  const head = teams?.rows.find((r) => /^team:[^:]+$/.test(r.id));
  if (!head) return null;
  const role = head.value?.split(" · ")[0];
  return role ? say("settings.account.team_line", { team: head.label, role }) : head.label;
}

/** One bar of the usage meter: `used` of `limit` (no bar without a limit). A tooltip on the bar says what remote hours are (the core's `relay_usage::REMOTE_HOURS_MEANING`), not a paragraph. */
function UsageBar({ id, label, text, used, limit, help }: { id: string; label: string; text: string; used: number; limit: number | null; help?: string }) {
  if (!limit || limit <= 0) return null;
  const share = Math.min(1, used / limit);
  return (
    <div className="px-4 py-3" data-usage-line={id} title={help}>
      <div className="mb-2 flex items-baseline justify-between gap-4 text-[13px]">
        <span>{label}</span>
        <span className="text-muted-foreground">{text}</span>
      </div>
      <div
        role="meter"
        aria-label={label}
        aria-valuemin={0}
        aria-valuemax={limit}
        aria-valuenow={Math.min(used, limit)}
        data-usage-meter={id}
        data-full={used >= limit ? "" : undefined}
        className="h-1.5 w-full overflow-hidden rounded-full bg-muted"
      >
        <div className={used >= limit ? "h-full bg-destructive" : "h-full bg-brand-strong"} style={{ width: `${Math.round(share * 100)}%` }} />
      </div>
    </div>
  );
}

/** What the plan gives, in one line, from the limits the relay reports. */
function allowanceLine(plan: string, usage: RelayUsage, say: ReturnType<typeof useT>): string | null {
  const { liveSessionsLimit: machines, hoursLimit: away } = usage;
  if (!machines || !away) return null;
  return say("settings.account.allowance", { plan, machines, hours: hours(away) });
}

/**
 * Settings, Account: who you are, your plan, what you used this month, and
 * the web account for teams, invites and billing (one button). Free accounts
 * also get Upgrade here. Every plan word comes from the relay's plan card.
 */
export function AccountTab() {
  const t = useT();
  const { data: settings, isLoading } = useSettings();
  const { data: session, signIn, signOut, cancelSignIn, openExternal } = useSession();
  const view = useRelayUsageView();
  const usage = useRelayUsage();
  const trackClick = useTrackPlanClick();
  if (isLoading && !settings) return null;

  const page = settings?.page;
  const account = page?.sections.find((s) => s.id === "account");
  const teams = teamLine(
    page?.sections.find((s) => s.id === "teams"),
    t,
  );
  const manageUrl =
    page?.sections.find((s) => s.id === "teams")?.rows.find((r) => r.id === "teams-manage")?.linkUrl ?? `${settings?.accountUrl ?? "https://app.cua.ai"}/team`;
  const card = view?.planCard;
  const upgrade = card?.plan === "free" ? card.primary : null;
  const open = (url: string) => void openExternal(url).catch(fail(t("settings.account.open_failed")));

  const press = (row: CoreRow) => {
    switch (row.id) {
      case "account":
        return void signOut().catch(fail(t("settings.account.sign_out_failed")));
      case "sign-in":
        return void signIn().catch(fail(t("settings.account.sign_in_failed")));
      case "sign-in-code":
        return cancelSignIn();
    }
  };

  return (
    <div data-settings-tab="account">
      <SettingsGroup title={t("settings.account.you")}>
        {account ? (
          account.rows.map((r) => <CoreSettingsRow key={r.id} row={r} onPress={press} />)
        ) : (
          <SettingsRow
            label={session?.signedIn ? (session.identity ?? t("settings.account.signed_in")) : t("settings.account.signed_out")}
            control={
              session?.signedIn ? (
                <Button size="sm" variant="outline" onClick={() => void signOut().catch(fail(t("settings.account.sign_out_failed")))}>
                  {t("settings.account.sign_out")}
                </Button>
              ) : (
                <Button size="sm" variant="outline" disabled={!session} onClick={() => void signIn().catch(fail(t("settings.account.sign_in_failed")))}>
                  {t("settings.account.sign_in")}
                </Button>
              )
            }
          />
        )}
        {view ? (
          <SettingsRow
            label={t("settings.account.plan")}
            control={<span className="text-[13px] text-muted-foreground">{planName(view.plan || card?.label || "")}</span>}
          />
        ) : null}
        {teams ? <SettingsRow label={t("settings.account.team")} control={<span className="text-[13px] text-muted-foreground">{teams}</span>} /> : null}
      </SettingsGroup>

      {usage && (usage.liveSessionsLimit || usage.hoursLimit) ? (
        <SettingsGroup title={t("settings.account.this_month")}>
          <div data-relay-usage="">
            <UsageBar
              id="Remote hours"
              label={t("settings.account.hours_label")}
              help={t("settings.account.hours_meaning")}
              text={t("settings.account.hours_of", { used: hours(usage.hoursUsed), limit: String(usage.hoursLimit ?? 0) })}
              used={usage.hoursUsed}
              limit={usage.hoursLimit}
            />
            <UsageBar
              id="Computers in use"
              label={t("settings.account.computers_label")}
              text={t("settings.account.computers_of", { used: String(usage.liveSessions), limit: String(usage.liveSessionsLimit ?? 0) })}
              used={usage.liveSessions}
              limit={usage.liveSessionsLimit}
            />
            {card ? (
              <p data-plan-card={card.plan} className="px-4 pb-3 text-xs text-muted-foreground">
                {allowanceLine(planName(card.label), usage, t)}
              </p>
            ) : null}
          </div>
        </SettingsGroup>
      ) : card ? (
        <p data-plan-card={card.plan} className="mb-8 px-1 text-xs text-muted-foreground">
          {card.label}
        </p>
      ) : null}

      <div className="flex flex-col gap-3">
        {upgrade ? (
          <Button
            className="h-12 w-full text-[13px]"
            data-plan-primary=""
            onClick={() => {
              trackClick("plan_card");
              open(upgrade.url);
            }}
          >
            {t("settings.account.upgrade")}
          </Button>
        ) : null}
        <Button variant={upgrade ? "outline" : "default"} className="h-12 w-full text-[13px]" data-manage-account="" onClick={() => open(manageUrl)}>
          {t("settings.account.manage")}
        </Button>
      </div>
    </div>
  );
}
