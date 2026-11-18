// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute } from "@tanstack/react-router";
import { WorkflowIcon } from "lucide-react";

import { useExperimentFlags } from "@/bridge";
import { CiPage } from "@/components/ci/ci-page";
import { EmptyState, Page, PageHeader } from "@/components/page";

export const Route = createFileRoute("/ci/")({ component: CiRoute });

/** The CI Runners page, shown only with the CI runners experiment on. */
function CiRoute() {
  const experiments = useExperimentFlags();
  if (experiments && !experiments.ciRunners) {
    return (
      <Page className="max-w-3xl">
        <PageHeader title="CI Runners" />
        <EmptyState icon={<WorkflowIcon />} title="CI runners is off">
          It's an experiment for now. Turn it on with cua experiments enable ci-runners.
        </EmptyState>
      </Page>
    );
  }
  return <CiPage />;
}
