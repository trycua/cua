// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useState } from "react";

import {
  agentName,
  agentSubtitle,
  AGENT_STATUS_DOT,
  AGENT_STATUS_LABEL,
  filterAgentRuns,
  orderAgentRuns,
  type SpaceAgentRun,
} from "../model/agents";
import { detailCopy } from "../model/window";
import type { TeleportBridge } from "../native/teleport";
import { AgentGlyph } from "./NavGlyphs";

/**
 * The AGENTS section of a Space's page: one row per coding-agent run inside it.
 *
 * It sits between Stream and Teleport under the page's section title (the
 * app core's words, one label per thing), and its rows reuse the window rows' geometry — the 46pt
 * 16:10 `.swl-row-preview` slot, a title, a status dot — so the three sections
 * read as one component family rather than three designs.
 *
 * Per row: a thumbnail for the agent TYPE (a glyph, since a headless agent has
 * no window to capture), the agent's name over a one-line summary of the prompt
 * it was given, and a status dot.
 *
 * HONESTY RULES, which are most of why this component is longer than the markup
 * it renders:
 *
 *  * a failed listing is NOT an empty list. "No agents are running" and "we
 *    could not ask" are different claims, and only one of them is ours to make;
 *  * a status the shell could not determine renders as Unknown, with the
 *    shell's own reason as the tooltip, rather than being rounded to idle; and
 *  * a run whose record could not be read is still a row. It is the one most
 *    worth seeing.
 *
 * Rows are not interactive yet: there is no steer/stop action wired to them, so
 * they are presented as a list rather than as buttons that would do nothing.
 */

/** How often the rows re-read. Slow enough to be free, quick enough that a
 *  finished agent stops saying "Running" while you are looking at it. */
const POLL_MS = 4000;

type Load =
  | { kind: "loading" }
  | { kind: "ready"; runs: SpaceAgentRun[] }
  | { kind: "failed"; message: string };

export function SpaceAgentList({
  spaceId,
  teleport,
  query = "",
}: {
  spaceId: string;
  teleport: TeleportBridge;
  query?: string;
}) {
  const copy = detailCopy();
  const [load, setLoad] = useState<Load>({ kind: "loading" });

  useEffect(() => {
    let cancelled = false;
    setLoad({ kind: "loading" });

    const read = () => {
      teleport
        .listSpaceAgents(spaceId)
        .then((runs) => {
          if (!cancelled) setLoad({ kind: "ready", runs });
        })
        .catch((error: unknown) => {
          if (cancelled) return;
          // Logged as well as shown: a section that says it could not read
          // must leave something behind that says why.
          console.warn(`[Cua Spaces] listing ${spaceId}'s agents failed`, error);
          setLoad({
            kind: "failed",
            message: error instanceof Error ? error.message : String(error),
          });
        });
    };

    read();
    const timer = setInterval(read, POLL_MS);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [teleport, spaceId]);

  const runs = load.kind === "ready" ? orderAgentRuns(filterAgentRuns(load.runs, query)) : [];

  return (
    <section className="swl-section swl-agents">
      {load.kind === "loading" ? (
        <p className="swl-empty" role="status">
          {copy.agentsLoading}
        </p>
      ) : load.kind === "failed" ? (
        /* Deliberately not "no agents": we do not know that. */
        <p className="swl-empty" role="status">
          {copy.agentsFailed}
        </p>
      ) : runs.length === 0 ? (
        <p className="swl-empty" role="status">
          {load.runs.length > 0 ? copy.agentsNoMatch : copy.agentsEmpty}
        </p>
      ) : (
        <ul className="swl-rows" aria-label="Agents in this Space">
          {runs.map((run) => (
            <li key={run.runId}>
              {/* The reason comes from the shell, so the tooltip explains the
                  dot rather than restating it. */}
              <div className="swl-row swl-row-agent" title={run.reason}>
                {/* The harness's real mark, the way a window row carries its
                    app's icon. An unrecognised harness renders an empty slot
                    rather than a stand-in glyph: a shared fallback drawn in
                    every unknown row identifies nothing, and makes two
                    different harnesses look like the same one. */}
                <span
                  className="swl-row-preview swl-row-preview-agent"
                  data-agent={run.agent || "unknown"}
                  aria-hidden="true"
                >
                  <AgentGlyph agent={run.agent} className="swl-agent-glyph" />
                </span>
                <span className="swl-row-title" title={agentSubtitle(run)}>
                  {agentName(run.agent)}
                </span>
                <span
                  className={`status-dot ${AGENT_STATUS_DOT[run.status]}`}
                  role="img"
                  aria-label={AGENT_STATUS_LABEL[run.status]}
                  title={AGENT_STATUS_LABEL[run.status]}
                />
              </div>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
