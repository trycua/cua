// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute, redirect } from "@tanstack/react-router";

import { flagOn } from "@/bridge";
import { AskFirstTask } from "@/components/ask-first/task-view";

/** One task's conversation on the "Ask first" Home (`spaces_ask_first_home`). */
export const Route = createFileRoute("/home/$taskId")({
  beforeLoad: () => {
    if (!flagOn("spaces_ask_first_home")) throw redirect({ to: "/spaces" });
  },
  component: TaskPage,
});

function TaskPage() {
  const { taskId } = Route.useParams();
  return <AskFirstTask taskId={taskId} />;
}
