// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute } from "@tanstack/react-router";

import { SpacesTreePage } from "@/components/spaces/spaces-tree-page";
import { validateComputersSearch } from "@/lib/old-links";

/**
 * Your computers and the Spaces made on them, in one list (CUA-3413,
 * CUA-3551). `?add=1` opens "Add a computer"; `?join=<code>` opens it on
 * "Enter a code" with the code filled in (a `cua://add/<code>` link opened on
 * this computer); `relay` names a relay other than the production one.
 */
export const Route = createFileRoute("/spaces/")({
  validateSearch: validateComputersSearch,
  component: SpacesRoute,
});

function SpacesRoute() {
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  return <SpacesTreePage search={search} onSearchDone={() => void navigate({ search: {}, replace: true })} />;
}
