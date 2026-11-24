// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { ServerIcon } from "lucide-react";

import { useCiRunners } from "@/bridge";
import { EmptyState, Page, PageHeader } from "@/components/page";
import { Button } from "@/components/ui/button";
import { GroupedRow, GroupedSection } from "@/components/ui/grouped";
import { Autopilot } from "./autopilot";
import { ConnectSheet } from "./connect-sheet";
import { Help } from "./help";
import { JobsTable } from "./jobs-table";
import { Repositories } from "./repositories";
import { SetupChecklist } from "./setup-checklist";
import { StatsLine } from "./stats-line";

/**
 * CI Runners: GitHub Actions jobs on your own computers. Setup first (until
 * it is complete), then the overview line, the jobs table, the repositories,
 * the automatic sizing of each cloud and a short help. The page keeps no rules of
 * its own: `useCiRunners` reads `cua ci overview` and draws what the app core
 * says (`ciRunners.view`).
 */
export function CiPage() {
  const ci = useCiRunners();
  const { view } = ci;

  if (ci.unsupported || ci.noCore) {
    return (
      <Page className="max-w-3xl">
        <PageHeader title="CI Runners" />
        <EmptyState icon={<ServerIcon />} title="Not available here">
          This window can't reach Cua CI. Open it from the Cua Spaces app.
        </EmptyState>
      </Page>
    );
  }
  if (!view) {
    return (
      <Page className="max-w-3xl">
        <PageHeader title="CI Runners" />
        {ci.error ? (
          <EmptyState icon={<ServerIcon />} title="Could not read CI Runners">
            {ci.error}
          </EmptyState>
        ) : (
          <div aria-busy="true" className="h-40" />
        )}
      </Page>
    );
  }

  const onSetupAction = (id: string) => {
    if (id === "connect") ci.act({ type: "connectOpen" });
    else if (id === "choose-repositories") ci.act({ type: "picker", open: true });
    else if (id === "copy-label") void navigator.clipboard?.writeText(view.runsOn).catch(() => undefined);
  };

  return (
    <Page className="max-w-3xl">
      <PageHeader
        title={view.title}
        description={view.intro}
        actions={
          view.connectButton ? (
            <Button data-ci-connect-button="" onClick={() => ci.act({ type: "connectOpen" })}>
              {view.connectButton.label}
            </Button>
          ) : null
        }
      />
      <div data-ci-page="" className="flex flex-col gap-7">
        {view.note ? (
          <p className="px-1 text-xs text-muted-foreground" data-ci-note="">
            {view.note}
          </p>
        ) : null}
        {view.setup ? <SetupChecklist setup={view.setup} onAction={onSetupAction} /> : null}
        <StatsLine stats={view.stats} />
        <JobsTable jobs={view.jobs} logs={ci.logs} onAction={ci.act} onOpen={ci.open} onShowLog={ci.showLog} />
        <Repositories picker={view.repositories} open={view.pickerOpen} onAction={ci.act} onSwitch={ci.setRepository} onOpen={ci.open} />
        {view.sizing.length > 0 ? (
          view.sizing.map((p) => <Autopilot key={p.target} pilot={p} heading={view.sizingTitle} cloudLabel={view.cloudLabel} onGoal={ci.setGoal} />)
        ) : (
          <GroupedSection title={view.sizingTitle} data-ci-no-cloud="">
            <GroupedRow className="text-muted-foreground">{view.noCloud}</GroupedRow>
          </GroupedSection>
        )}
        {ci.notice ? (
          <p className="-mt-3 px-1 text-xs text-muted-foreground" role="status" data-ci-notice="">
            {ci.notice}
          </p>
        ) : null}
        <Help title={view.helpTitle} topics={view.help} onAction={ci.act} />
      </div>
      {view.connect ? <ConnectSheet view={view.connect} onAction={ci.act} onOpen={ci.open} /> : null}
    </Page>
  );
}
