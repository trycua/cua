// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { FleetTemplate, TemplateId } from "./types";

export const TEMPLATES: FleetTemplate[] = [
  {
    id: "qa-matrix",
    name: "QA Matrix",
    summary: "Windows + Linux + macOS · 3 computers",
    description:
      "One computer per desktop OS so an agent can run the same test plan everywhere and diff the results.",
    members: [
      { name: "Windows QA", os: "windows", scene: "windows-desktop" },
      { name: "Linux QA", os: "linux", scene: "linux-terminal" },
      { name: "macOS Verify", os: "macos", scene: "mac-desktop" },
    ],
    adjustable: false,
  },
  {
    id: "parallel-build",
    name: "Parallel Build",
    summary: "3 Linux builders",
    description:
      "Identical Linux builders that split a build or test suite into shards. Add more to shorten the critical path.",
    members: [
      { name: "Builder 1", os: "linux", scene: "linux-terminal" },
      { name: "Builder 2", os: "linux", scene: "linux-terminal" },
      { name: "Builder 3", os: "linux", scene: "linux-terminal" },
    ],
    adjustable: true,
  },
  {
    id: "clean-browsers",
    name: "Clean Browsers",
    summary: "3 isolated browser computers",
    description:
      "Fresh browser profiles on isolated computers for sign-up flows, A/B checks, and anything that must not share cookies.",
    members: [
      { name: "Browser A", os: "linux", scene: "browser" },
      { name: "Browser B", os: "linux", scene: "browser" },
      { name: "Browser C", os: "linux", scene: "browser" },
    ],
    adjustable: true,
  },
  {
    id: "custom",
    name: "Custom",
    summary: "Choose systems and sizes",
    description: "Start from a blank group and pick the operating system and computer count yourself.",
    members: [],
    adjustable: true,
  },
];

export function getTemplate(id: TemplateId): FleetTemplate {
  const found = TEMPLATES.find((t) => t.id === id);
  if (!found) throw new Error(`Unknown template: ${id}`);
  return found;
}
