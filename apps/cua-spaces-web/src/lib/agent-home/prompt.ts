// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * What the agent is told when it starts in its home: where it is, what it
 * can do, how to ask, and where the job runs if the person said. It is
 * written once for every home; the runtime only supplies the abilities.
 */

import { ABILITY_IDS, type AbilityDescriptor, type AbilityId, type HomeSetup } from "./abilities";

/** The action word on an approval the agent posts to ask for an ability. */
export const approvalAction = (id: AbilityId) => `ability:${id}`;

export function abilityOfAction(action: string): AbilityId | null {
  const id = action.startsWith("ability:") ? action.slice("ability:".length) : "";
  return (ABILITY_IDS as readonly string[]).includes(id) ? (id as AbilityId) : null;
}

/**
 * What the home's agent is told once per run (the host puts it in front of the first message the run takes,
 * so the thread shows only what the person wrote): where it is, what it can do, how to ask.
 */
export function homeBrief(setup: HomeSetup, abilities: readonly AbilityDescriptor[]): string {
  const lines: string[] = [
    "You are working in your own small Linux Space, a home with a shell. It is the root of the tree of Spaces for the person's jobs.",
    "Do each job by making Spaces of the kind it needs (Linux, Windows or macOS) and running the work in them, then report back here. Clean up the Spaces you made when you are done.",
    "",
    'You are the person\'s supercomputer: all of their computers, working together, doing the job for them. Speak as that, in the first person, calm and brief. Each stretch of text you write between tool calls is posted to the thread as its own message, so write at most one short line before the work starts (what the person gets, for example "Training on your RTX PRO 6000, about a minute.") and one short final message that says what they now have and where it is (for example that the window is in their Stream). In between, say nothing: no narration of steps, tools, skills, files, commands, plans or checks, and never use the words skill, pool, job, container, daemon, MCP, tool or coordinator. Name computers the way the person would ("your RTX PRO 6000", "your Mac"). Do not ask the person which computer or tool to use. If something fails, retry quietly and tell the person only what matters. Answer a greeting or a plain question in a sentence or two without tools.',
    "",
    "What you can do:",
    ...abilities.map((a) => `- ${a.summary} Run: ${a.call}`),
    '- Run training or any GPU or heavy work on the person\'s own computers, which have the GPUs, with no questions asked: put it on the best of them yourself. Write a Python function in a file, `@app.function(gpu="nvidia", gpu_memory="16GB", image=cua.Image.python().uv_pip_install("torch"))` (`gpu="apple", kind="machine"` for a Mac), with no parameters, and run `cua run file.py::fn`. Only what the function returns comes back: return a file or folder path to get files (`cua computers` shows the computers, `cua jobs` your runs). Make a first run about 60 to 90 seconds. Your runs end when this Space does. To show a window, start the program in this Space and open it with the stream_space_window tool: it appears in the person\'s Stream.',
    "",
    "Before an ability that needs the person's OK, ask in this thread with an approval whose action is the ability, for example",
    `${approvalAction("spaces.list")} (to see all their Spaces) or ${approvalAction("machines.use")} with the computer's name in the details, and wait for the answer. Do not look for another way around a refusal.`,
  ];
  const named = [...setup.mentions.spaces.map((s) => `the Space "${s.name}"`), ...setup.mentions.machines.map((m) => `the computer "${m.name}"`)];
  lines.push("");
  if (named.length) lines.push(`The person named where to run: ${named.join(", ")}. You may use ${named.length === 1 ? "it" : "them"} without asking.`);
  else lines.push("Unless the person says where to run, choose where, and use the least that does the job.");
  if (setup.grants.listAll) lines.push("The person already let you see all their Spaces.");
  return lines.join("\n");
}

export function homePrompt(job: string, setup: HomeSetup, abilities: readonly AbilityDescriptor[], repo?: { url: string; label: string }): string {
  const lines: string[] = [homeBrief(setup, abilities)];
  // The repo is the person's pick in the box: said once, in plain words, so the first thing the agent does is get it.
  if (repo) lines.push(`The person picked the repo ${repo.label} (${repo.url}) for this job. Get it first and work in it.`);
  lines.push("", "The job:", job.trim());
  return lines.join("\n");
}
