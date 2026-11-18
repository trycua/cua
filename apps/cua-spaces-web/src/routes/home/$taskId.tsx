// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute } from "@tanstack/react-router";

import { AskFirstTask } from "@/components/ask-first/task-view";

/** One task's conversation on Home. */
export const Route = createFileRoute("/home/$taskId")({
  component: TaskPage,
});

function TaskPage() {
  const { taskId } = Route.useParams();
  return <AskFirstTask taskId={taskId} />;
}
