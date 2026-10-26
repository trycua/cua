// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { getRouteApi } from "@tanstack/react-router";

import { KeyvaultPage } from "@/legacy/components/keyvault/keyvault-page";


const Route = getRouteApi("/keyvault/");

/** Public's page before the redesign (`spaces_redesign` off); its search is the real route's. */
export const LegacyKeyvaultPage = KeyvaultRoute;

function KeyvaultRoute() {
  const { view, focus } = Route.useSearch();
  return <KeyvaultPage initialView={view} focus={focus ?? null} />;
}
