/**
 * Record the exact TypeSafe Jev request payloads for the page and visual
 * fixtures.
 *
 * fixtures/jev-provider-request-golden-typescript-v1.json was captured from this
 * module on main before the candidate-source and task-spec refactor (RFC #4268
 * Phase 0). provider_request_golden.test.ts replays the same scenarios and
 * requires byte-identical payloads, so any change to the model input or the
 * candidate table is a test failure, not a silent drift.
 *
 * Regenerate only for an intentional contract change:
 *
 *   npx tsx typescript/provider_request_golden.ts --write
 */
import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync } from 'node:fs';
import process from 'node:process';
import { pathToFileURL } from 'node:url';

import {
  SUBMIT_IDS,
  buildCandidates,
  hasExecutableCandidate,
  historyEntry,
  parseVisualRegions,
  validateChoice,
  type BrowserSnapshot,
  type Candidate,
  type HistoryEntry,
  type VisualDelivery,
  type VisualObservation,
} from './core.js';
import { chooseWithTypeSafe } from './jev_adapter.js';

export const GOLDEN = new URL(
  '../fixtures/jev-provider-request-golden-typescript-v1.json',
  import.meta.url
);

const load = (name: string) =>
  JSON.parse(readFileSync(new URL(`../fixtures/${name}`, import.meta.url), 'utf8'));
const PAGE = load('jev-page-structure-replay-v1.json');
const VISUAL = load('jev-visual-replay-v1.json');

export type Build = (
  snapshot: BrowserSnapshot,
  token: string,
  visual: VisualObservation | undefined,
  captureBoundClick: boolean,
  visualDelivery: VisualDelivery
) => Candidate[];

export type Choose = (
  client: RecordingClient,
  candidates: Candidate[],
  snapshot: BrowserSnapshot,
  visual: VisualObservation | undefined,
  history: readonly HistoryEntry[],
  token: string,
  visualPath: boolean
) => Promise<{ choice: string }>;

type Fixture = {
  token: string;
  snapshots: { before_typing: BrowserSnapshot; after_typing: BrowserSnapshot };
  recorded_live_runs: Record<string, { steps: RecordedStep[] }>;
};

type RecordedStep = { step?: number; selected_id: string; action_error?: string };

type WireRecord = { step?: number; request: unknown; candidates: unknown };

export function observation(payload = VISUAL.visual_regions): VisualObservation {
  const source = payload.capture.source;
  return parseVisualRegions(payload, payload.capture.capture_id, source.pid, source.window_id);
}

/**
 * Answer with scripted IDs and keep every request exactly as sent. A scripted
 * ID absent from the offered criteria (for example a visual Submit when visual
 * parsing is off) is answered with reobserve.
 */
export class RecordingClient {
  readonly requests: any[] = [];
  private readonly choices: string[];

  constructor(choices: string[]) {
    this.choices = [...choices];
  }

  async systemOne(request: any) {
    this.requests.push(request);
    let selected = this.choices.shift()!;
    if (!Object.hasOwn(request.questions.driver_action.criteria, selected)) selected = 'reobserve';
    return {
      answers: {
        driver_action: {
          type: 'choice' as const,
          choice: selected,
          confidence: 1,
          probabilities: { [selected]: 1 },
        },
      },
    };
  }
}

function wireCandidates(candidates: readonly Candidate[]) {
  return candidates.map((candidate) => ({
    id: candidate.id,
    description: candidate.description,
    tool: candidate.tool,
    arguments: JSON.parse(JSON.stringify(candidate.arguments)),
    capture_id: candidate.captureId ?? null,
    screenshot_reference: candidate.screenshotReference ?? null,
  }));
}

function wireRequest(request: any) {
  return JSON.parse(JSON.stringify(request));
}

async function replay(
  build: Build,
  choose: Choose,
  fixture: Fixture,
  run: { steps: RecordedStep[] },
  captureBoundClick: boolean,
  visualMode: 'auto' | 'always' | 'off'
): Promise<WireRecord[]> {
  const token = fixture.token;
  const before = fixture.snapshots.before_typing;
  const after = fixture.snapshots.after_typing;
  const visualPath = captureBoundClick && visualMode !== 'off';
  const history: HistoryEntry[] = [];
  let typed = false;
  let delivery: VisualDelivery = 'background';
  const records: WireRecord[] = [];
  const client = new RecordingClient(run.steps.map((step) => step.selected_id));
  for (const [offset, recorded] of run.steps.entries()) {
    const step = recorded.step ?? offset + 1;
    const snapshot = typed ? after : before;
    let candidates = build(snapshot, token, undefined, captureBoundClick, delivery);
    let visual: VisualObservation | undefined;
    if (
      visualMode === 'always' ||
      (visualMode === 'auto' && !hasExecutableCandidate(candidates))
    ) {
      if (captureBoundClick) {
        visual = observation();
        candidates = build(snapshot, token, visual, captureBoundClick, delivery);
      }
    }
    const answer = await choose(client, candidates, snapshot, visual, history, token, visualPath);
    const candidate = validateChoice(answer.choice, candidates, visual?.captureId);
    records.push({
      step,
      request: wireRequest(client.requests.at(-1)),
      candidates: wireCandidates(candidates),
    });
    const refusal = recorded.action_error;
    history.push(historyEntry(step, candidate.id, refusal));
    if (candidate.id === 'abstain') break;
    if (candidate.id === 'type-verification-value') {
      typed = true;
    } else if (SUBMIT_IDS.has(candidate.id)) {
      if (refusal) delivery = 'foreground';
      else break;
    }
  }
  return records;
}

function withFieldValue(snapshot: BrowserSnapshot, value: string | null): BrowserSnapshot {
  const changed = structuredClone(snapshot);
  for (const ref of changed.refs ?? []) {
    if (ref.role === 'textbox' && ref.name === 'verification value') ref.value = value;
  }
  return changed;
}

function withoutButton(snapshot: BrowserSnapshot): BrowserSnapshot {
  const changed = structuredClone(snapshot);
  changed.refs = (changed.refs ?? []).filter((ref) => ref.role !== 'button');
  return changed;
}

async function singleRequests(build: Build, choose: Choose) {
  const pageToken: string = PAGE.token;
  const pageBefore: BrowserSnapshot = PAGE.snapshots.before_typing;
  const pageAfter: BrowserSnapshot = PAGE.snapshots.after_typing;
  const visualToken: string = VISUAL.token;
  const visualAfter: BrowserSnapshot = VISUAL.snapshots.after_typing;
  const visual = observation();
  const noSubmit = { ...visual, regions: visual.regions.filter((r) => r.text !== 'Submit') };
  const submit = visual.regions.find((r) => r.text === 'Submit')!;
  const duplicated = { ...visual, regions: [...visual.regions, { ...submit, id: 'dup' }] };
  const empty: BrowserSnapshot = { target_id: 'target', tab_id: 'tab', refs: [] };
  const cases: Record<
    string,
    [BrowserSnapshot, string, VisualObservation | undefined, boolean, boolean, VisualDelivery]
  > = {
    page_other_value: [withFieldValue(pageBefore, 'other'), pageToken, undefined, false, false, 'background'],
    page_after_with_visual: [pageAfter, pageToken, visual, true, true, 'background'],
    page_after_no_button: [withoutButton(pageAfter), pageToken, undefined, false, false, 'background'],
    page_after_no_button_visual_path: [withoutButton(pageAfter), pageToken, undefined, true, true, 'background'],
    no_form_refs: [empty, pageToken, undefined, false, false, 'background'],
    visual_after_no_capture_bound_click: [visualAfter, visualToken, visual, false, true, 'background'],
    visual_after_without_submit_region: [visualAfter, visualToken, noSubmit, true, true, 'background'],
    visual_after_duplicate_submit_region: [visualAfter, visualToken, duplicated, true, true, 'background'],
    visual_after_foreground: [visualAfter, visualToken, visual, true, true, 'foreground'],
    visual_after_other_value: [withFieldValue(visualAfter, 'other'), visualToken, visual, true, true, 'background'],
  };
  const result: Record<string, WireRecord> = {};
  for (const [name, [snapshot, token, current, cbc, visualPath, delivery]] of Object.entries(cases)) {
    const candidates = build(snapshot, token, current, cbc, delivery);
    const client = new RecordingClient(['abstain']);
    await choose(client, candidates, snapshot, current, [], token, visualPath);
    result[name] = {
      request: wireRequest(client.requests.at(-1)),
      candidates: wireCandidates(candidates),
    };
  }
  return result;
}

function digest(value: unknown): string {
  return createHash('sha256').update(JSON.stringify(value), 'utf8').digest('hex').slice(0, 16);
}

/**
 * Store each distinct request and candidate table once, keyed by digest.
 * Scenario records keep their order and reference the exact payloads.
 */
function deduplicate(
  runs: Record<string, WireRecord[]>,
  singles: Record<string, WireRecord>
) {
  const requests: Record<string, unknown> = {};
  const tables: Record<string, unknown> = {};
  const reference = (record: WireRecord) => {
    const requestId = digest(record.request);
    const tableId = digest(record.candidates);
    requests[requestId] ??= record.request;
    tables[tableId] ??= record.candidates;
    const refs = { request: requestId, candidates: tableId };
    return record.step === undefined ? refs : { step: record.step, ...refs };
  };
  return {
    schema: 'cua.jev_use_provider_request_golden_v1',
    language: 'typescript',
    runs: Object.fromEntries(
      Object.entries(runs).map(([name, records]) => [name, records.map(reference)])
    ),
    single_requests: Object.fromEntries(
      Object.entries(singles).map(([name, record]) => [name, reference(record)])
    ),
    requests,
    candidate_tables: tables,
  };
}

export async function payloads(build: Build, choose: Choose) {
  const modes: [string, Fixture, boolean, 'auto' | 'always' | 'off'][] = [
    ['page', PAGE, false, 'auto'],
    ['page_capture_bound_auto', PAGE, true, 'auto'],
    ['page_capture_bound_always', PAGE, true, 'always'],
    ['visual_capture_bound_auto', VISUAL, true, 'auto'],
    ['visual_capture_bound_always', VISUAL, true, 'always'],
    ['visual_capture_bound_off', VISUAL, true, 'off'],
  ];
  const runs: Record<string, WireRecord[]> = {};
  for (const [prefix, fixture, cbc, mode] of modes) {
    for (const [name, run] of Object.entries(fixture.recorded_live_runs)) {
      runs[`${prefix}/${name}`] = await replay(build, choose, fixture, run, cbc, mode);
    }
  }
  return deduplicate(runs, await singleRequests(build, choose));
}

export const legacyBuild: Build = (snapshot, token, visual, captureBoundClick, visualDelivery) =>
  buildCandidates(snapshot, token, visual, captureBoundClick, visualDelivery);

export const legacyChoose: Choose = (
  client,
  candidates,
  snapshot,
  visual,
  history,
  token,
  visualPath
) =>
  chooseWithTypeSafe(client as never, candidates, snapshot, visual, history, token, visualPath);

export function encode(value: unknown): string {
  return `${JSON.stringify(value, null, 1)}\n`;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const text = encode(await payloads(legacyBuild, legacyChoose));
  if (process.argv.includes('--write')) writeFileSync(GOLDEN, text, 'utf8');
  else process.stdout.write(text);
}
