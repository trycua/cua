// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Link, createFileRoute, useNavigate } from "@tanstack/react-router";
import { CloudIcon } from "lucide-react";

import { useMachines } from "@/bridge";
import { BackLink } from "@/components/back-link";
import { CloudDetail } from "@/components/clouds/cloud-detail";
import { EmptyState, Page } from "@/components/page";
import { Button } from "@/components/ui/button";
import { useT } from "@/i18n";

/** One connected cloud's page, opened from the Clouds list under your computers (CUA-3551). */
export const Route = createFileRoute("/spaces/cloud/$cloudId")({
  component: CloudRoute,
});

function CloudRoute() {
  const t = useT();
  const { cloudId } = Route.useParams();
  const { view, isLoading } = useMachines();
  const navigate = useNavigate();
  const page = view?.clouds?.pages.find((c) => c.id === cloudId);
  return (
    <Page className="max-w-3xl">
      <BackLink />
      {page ? (
        <CloudDetail key={page.id} page={page} onGone={() => void navigate({ to: "/spaces" })} />
      ) : isLoading || !view ? (
        <div aria-busy="true" aria-label={t("computers.page.cloud_loading")} className="h-28 animate-pulse rounded-lg bg-muted" />
      ) : (
        <EmptyState
          icon={<CloudIcon />}
          title={t("computers.page.cloud_missing_title")}
          action={<Button render={<Link to="/spaces" />}>{t("computers.page.missing_action")}</Button>}
        >
          {t("computers.page.cloud_missing_body")}
        </EmptyState>
      )}
    </Page>
  );
}
