// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useNavigate } from "@tanstack/react-router";

import { usePrepare } from "@/lib/prepare/store";

/** The failed view's "Add a computer" (`prepare::ADD_COMPUTER_ACTION`): this computer can't run Spaces itself. */
export const ADD_COMPUTER_ACTION = "add-computer";

/** What the failed preparing view's button does: "Add a computer" opens it, anything else tries again. */
export function usePrepareAction(): () => void {
  const prep = usePrepare();
  const navigate = useNavigate();
  return () => {
    if (prep.view?.action?.id === ADD_COMPUTER_ACTION) void navigate({ to: "/spaces", search: { add: true } });
    else prep.driver?.retry();
  };
}
