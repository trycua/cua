import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test, { mock } from 'node:test';
import { fileURLToPath } from 'node:url';

import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { TypeSafeClient } from '@typesafe-ai/sdk';
import { FixtureFormTask, type TaskSources } from './tasks.js';

const TOKEN = 'runner-private-verification-value';

type Scenario = {
  guarded?: boolean;
  mode?:
    | 'accepted'
    | 'already-verified'
    | 'budget'
    | 'reobserve'
    | 'abstain'
    | 'no-choice'
    | 'ref-reused'
    | 'visual-refusal';
  actionError?: boolean;
  providerError?: boolean;
  duplicateCandidate?: boolean;
};

async function runFixture(scenario: Scenario, log: string) {
  let value = '';
  let submitted: string | null = scenario.mode === 'already-verified' ? TOKEN : null;
  let observations = 0;
  let providerCalls = 0;
  const actions: Record<string, unknown>[] = [];
  const visual = JSON.parse(
    readFileSync(new URL('../fixtures/jev-visual-replay-v1.json', import.meta.url), 'utf8')
  ).visual_regions;

  if (scenario.mode === 'abstain') {
    Object.defineProperty(FixtureFormTask.prototype, 'mockPreferences', {
      value: ['type-verification-value', 'abstain'],
    });
  }
  if (scenario.mode === 'no-choice' || scenario.duplicateCandidate) {
    const original = FixtureFormTask.prototype.candidates;
    mock.method(
      FixtureFormTask.prototype,
      'candidates',
      function (this: FixtureFormTask, sources: TaskSources) {
        const candidates = original.call(this, sources);
        if (observations !== 2) return candidates;
        return scenario.duplicateCandidate
          ? [...candidates, candidates[0]]
          : candidates.filter((candidate) => candidate.id === 'abstain');
      }
    );
  }
  if (scenario.providerError) {
    process.env.TYPESAFE_API_KEY = 'test-only-key';
    mock.method(TypeSafeClient.prototype, 'systemOne', async () => {
      providerCalls += 1;
      if (providerCalls > 1) throw new Error(TOKEN);
      return {
        answers: {
          driver_action: {
            type: 'choice',
            choice: 'type-verification-value',
            confidence: 0.72,
            probabilities: { 'type-verification-value': 0.72, reobserve: 0.18, abstain: 0.1 },
          },
        },
      };
    });
  }

  mock.method(FixtureFormTask.prototype, 'reset', async () => {});
  mock.method(FixtureFormTask.prototype, 'readOracle', async () => ({ submitted }));
  mock.method(Client.prototype, 'connect', async () => {});
  mock.method(Client.prototype, 'listTools', async () => ({
    tools: [
      { name: 'click', inputSchema: { properties: { capture_id: {} } } },
      { name: 'get_window_state' },
      { name: 'parse_visual_regions' },
    ],
  }));
  mock.method(Client.prototype, 'close', async () => {
    console.error(JSON.stringify({ closed: true, actions, providerCalls }));
  });
  mock.method(
    Client.prototype,
    'callTool',
    async (request: { name: string; arguments: Record<string, unknown> }) => {
      const { name, arguments: args } = request;
      let data: Record<string, unknown> = {};
      if (name === 'browser_prepare') data = { prepared_pid: 42 };
      else if (name === 'list_windows') {
        data = {
          windows: [{ window_id: 7, is_on_screen: true, bounds: { width: 800, height: 600 } }],
        };
      } else if (name === 'get_browser_state') {
        if (args.snapshot_format !== 'semantic_v2') {
          data = { target_id: 'target', tabs: [{ tab_id: 'tab', active: true }] };
        } else {
          observations += 1;
          const noSubmit =
            (observations === 2 &&
              ['reobserve', 'abstain', 'no-choice'].includes(scenario.mode ?? '')) ||
            (observations >= 2 && scenario.mode === 'visual-refusal');
          const submitRef = scenario.mode === 'ref-reused' ? 'p1:1' : `p${observations}:1`;
          data = {
            target_id: 'target',
            tab_id: 'tab',
            refs: [
              { role: 'textbox', name: 'verification value', ref: `p${observations}:0`, value },
              ...(noSubmit ? [] : [{ role: 'button', name: 'Submit', ref: submitRef }]),
            ],
          };
        }
      } else if (name === 'get_window_state') {
        data = { capture_id: visual.capture.capture_id };
      } else if (name === 'parse_visual_regions') {
        data = {
          ...visual,
          capture: { ...visual.capture, source: { kind: 'window', pid: 42, window_id: 7 } },
        };
      } else if (name === 'browser_type' || name === 'browser_click' || name === 'click') {
        actions.push({
          tool: name,
          ref: args.ref,
          session: args.session,
          ...(name === 'click' ? { delivery_mode: args.delivery_mode } : {}),
        });
        if (
          (scenario.actionError && name === 'browser_click') ||
          (name === 'click' && args.delivery_mode === 'background')
        ) {
          return {
            isError: true,
            content: [{ type: 'text', text: TOKEN }],
            structuredContent: {
              code: name === 'click' ? 'background_not_supported' : 'action_failed',
            },
          };
        }
        if (name === 'browser_type') value = String(args.text);
        else submitted = value;
      } else if (name !== 'browser_navigate') {
        throw new Error(`unexpected Driver tool ${name}`);
      }
      return { content: [], structuredContent: data };
    }
  );

  process.argv = [
    process.execPath,
    fileURLToPath(new URL('./run.ts', import.meta.url)),
    '--provider',
    scenario.providerError ? 'live' : 'mock',
    '--visual-observation',
    scenario.mode === 'visual-refusal' ? 'auto' : 'off',
    '--token',
    TOKEN,
    '--max-steps',
    scenario.mode === 'budget' ? '1' : '4',
    '--log',
    log,
    ...(scenario.guarded === false ? [] : ['--guarded-completion']),
  ];
  await import('./run.js');
}

function runScenario(scenario: Scenario = {}) {
  const directory = mkdtempSync(join(tmpdir(), 'jev-guarded-run-'));
  try {
    const log = join(directory, 'events.jsonl');
    const child = spawnSync(
      process.execPath,
      [
        '--import',
        'tsx',
        fileURLToPath(import.meta.url),
        '--fixture-run',
        JSON.stringify(scenario),
        log,
      ],
      { encoding: 'utf8', timeout: 15_000 }
    );
    assert.equal(child.error, undefined);
    const jsonl = readFileSync(log, 'utf8');
    assert.equal(child.stdout, jsonl);
    const events: Record<string, any>[] = jsonl
      .trim()
      .split('\n')
      .filter(Boolean)
      .map((line) => JSON.parse(line));
    const receipt = JSON.parse(
      child.stderr
        .trim()
        .split('\n')
        .find((line) => line.startsWith('{')) ?? '{}'
    );
    assert.equal(receipt.closed, true, child.stderr);
    return { events, receipt, jsonl, status: child.status, stderr: child.stderr };
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

if (process.argv[2] === '--fixture-run') {
  await runFixture(JSON.parse(process.argv[3]), process.argv[4]);
} else {
  test('runner dispatches the accepted fresh candidate with proof telemetry, not model scores', () => {
    const { events, receipt, status, stderr } = runScenario();
    assert.equal(status, 0, stderr);
    const steps = events.filter((event) => event.event === 'step');
    assert.equal(steps.length, 2);
    assert.equal(steps[0].decision_route, 'provider');
    assert.equal(Object.hasOwn(steps[0], 'guarded_completion'), false);
    assert.equal(steps[0].confidence, 1);
    assert.deepEqual(steps[0].probabilities, {
      'type-verification-value': 1,
      reobserve: 0,
      abstain: 0,
    });
    assert.equal(steps[1].candidate, 'submit-form');
    assert.equal(steps[1].decision_route, 'guarded-completion');
    assert.equal(steps[1].confidence, null);
    assert.equal(steps[1].probabilities, null);
    assert.equal(steps[1].provider_decision_ms, 0);
    assert.deepEqual(steps[1].guarded_completion, {
      status: 'accepted',
      prior_ref: 'p1:1',
      fresh_ref: 'p2:1',
      verification_field: 'contains_required_token',
      submit_matches: 1,
      session: receipt.actions[0].session,
    });
    assert.deepEqual(receipt.actions, [
      { tool: 'browser_type', ref: 'p1:0', session: receipt.actions[0].session },
      { tool: 'browser_click', ref: 'p2:1', session: receipt.actions[0].session },
    ]);
    assert.equal(events.at(-1)?.outcome, 'verified');
  });

  for (const mode of ['accepted', 'already-verified', 'budget'] as const) {
    for (const guarded of [false, true]) {
      test(`runner ${mode} outcomes omit the verification token (guarded=${guarded})`, () => {
        const { events, jsonl, status } = runScenario({ mode, guarded });
        const outcome = mode === 'budget' ? 'budget_exhausted' : 'verified';
        assert.equal(status, mode === 'budget' ? 1 : 0);
        assert.deepEqual(events.at(-1), { event: 'outcome', outcome });
        assert.equal(jsonl.includes(TOKEN), false);
      });
    }
  }

  test('runner keeps the default-off provider path and scores unchanged', () => {
    const { events, receipt, status } = runScenario({ guarded: false });
    assert.equal(status, 0);
    const steps = events.filter((event) => event.event === 'step');
    assert.equal(steps.length, 2);
    for (const event of events) assert.equal(Object.hasOwn(event, 'guarded_completion'), false);
    for (const step of steps) {
      assert.equal(step.decision_route, 'provider');
      assert.equal(step.confidence, 1);
      assert.equal(step.probabilities[step.candidate], 1);
    }
    assert.equal(receipt.actions[1].ref, 'p2:1');
  });

  test('runner logs a declined reobserve step and consumes the pending proof once', () => {
    const { events, receipt, status } = runScenario({ mode: 'reobserve' });
    assert.equal(status, 0);
    const steps = events.filter((event) => event.event === 'step');
    assert.deepEqual(
      steps.map((step) => step.candidate),
      ['type-verification-value', 'reobserve', 'submit-form']
    );
    assert.deepEqual(steps[1].guarded_completion, {
      status: 'declined',
      reason: 'submit_not_unique',
    });
    assert.equal(steps[1].decision_route, 'provider');
    assert.equal(steps[1].confidence, 1);
    assert.equal(steps[2].decision_route, 'provider');
    assert.equal(Object.hasOwn(steps[2], 'guarded_completion'), false);
    assert.equal(receipt.actions[1].ref, 'p3:1');
  });

  for (const mode of ['abstain', 'no-choice'] as const) {
    test(`runner logs a declined ${mode} outcome`, () => {
      const { events, receipt, status, jsonl } = runScenario({ mode });
      assert.equal(status, 1);
      const outcome = events.at(-1)!;
      assert.equal(outcome.event, 'outcome');
      assert.equal(outcome.outcome, 'abstained');
      assert.equal(outcome.step, 2);
      assert.deepEqual(outcome.guarded_completion, {
        status: 'declined',
        reason: 'submit_not_unique',
      });
      assert.equal(receipt.actions.length, 1);
      assert.equal(jsonl.includes(TOKEN), false);
    });
  }

  test('runner retains declined telemetry on a normal provider action', () => {
    const { events, status } = runScenario({ mode: 'ref-reused' });
    assert.equal(status, 0);
    assert.deepEqual(events[1].guarded_completion, { status: 'declined', reason: 'ref_reused' });
    assert.equal(events[1].decision_route, 'provider');
    assert.equal(events[1].confidence, 1);
    assert.deepEqual(events[1].probabilities, { 'submit-form': 1, reobserve: 0, abstain: 0 });
  });

  for (const mode of ['accepted', 'ref-reused'] as const) {
    for (const guarded of [false, true]) {
      test(`runner logs action failure telemetry for ${mode} (guarded=${guarded})`, () => {
        const { events, receipt, status, jsonl } = runScenario({
          mode,
          guarded,
          actionError: true,
        });
        assert.equal(status, 1);
        const outcome = events.at(-1)!;
        assert.equal(outcome.event, 'outcome');
        assert.equal(outcome.outcome, 'unknown');
        assert.equal(outcome.step, 2);
        assert.equal(outcome.phase, 'action');
        assert.equal(outcome.error, 'DriverToolError');
        assert.equal(outcome.tool, 'browser_click');
        if (!guarded) assert.equal(Object.hasOwn(outcome, 'guarded_completion'), false);
        else
          assert.deepEqual(
            outcome.guarded_completion,
            mode === 'accepted'
              ? {
                  status: 'accepted',
                  prior_ref: 'p1:1',
                  fresh_ref: 'p2:1',
                  verification_field: 'contains_required_token',
                  submit_matches: 1,
                  session: receipt.actions[0].session,
                }
              : { status: 'declined', reason: 'ref_reused' }
          );
        assert.equal(receipt.actions.length, 2);
        assert.equal(jsonl.includes(TOKEN), false);
      });
    }
  }

  test('runner logs declined telemetry on background refusal without carrying it to foreground', () => {
    const { events, receipt, status } = runScenario({ mode: 'visual-refusal' });
    assert.equal(status, 0);
    const steps = events.filter((event) => event.event === 'step');
    assert.equal(steps.length, 3);
    assert.deepEqual(steps[1].guarded_completion, {
      status: 'declined',
      reason: 'submit_not_unique',
    });
    assert.equal(steps[1].action_error, 'background_not_supported');
    assert.equal(steps[1].decision_route, 'provider');
    assert.equal(steps[2].decision_route, 'provider');
    assert.equal(Object.hasOwn(steps[2], 'guarded_completion'), false);
    assert.deepEqual(
      receipt.actions.slice(1).map((action: Record<string, unknown>) => action.delivery_mode),
      ['background', 'foreground']
    );
  });

  for (const providerError of [false, true]) {
    test(`runner logs declined telemetry on provider ${providerError ? 'request' : 'validation'} failure`, () => {
      const { events, receipt, status, jsonl } = runScenario({
        mode: providerError ? 'ref-reused' : 'accepted',
        providerError,
        duplicateCandidate: !providerError,
      });
      assert.equal(status, 1);
      const outcome = events.at(-1)!;
      assert.deepEqual(outcome, {
        event: 'outcome',
        outcome: 'unknown',
        step: 2,
        phase: 'provider',
        decision_route: 'provider',
        error: 'Error',
        visual: { status: 'skipped', reason: 'disabled' },
        guarded_completion: {
          status: 'declined',
          reason: providerError ? 'ref_reused' : 'candidate_not_unique',
        },
      });
      if (providerError) {
        assert.equal(receipt.providerCalls, 2);
        assert.equal(events[0].confidence, 0.72);
        assert.deepEqual(events[0].probabilities, {
          'type-verification-value': 0.72,
          reobserve: 0.18,
          abstain: 0.1,
        });
      }
      assert.equal(receipt.actions.length, 1);
      assert.equal(jsonl.includes(TOKEN), false);
    });
  }

  test('runner preserves default-off provider failure behavior without guard telemetry', () => {
    const { events, receipt, status, jsonl } = runScenario({ guarded: false, providerError: true });
    assert.equal(status, 1);
    assert.equal(events.length, 1);
    assert.equal(events[0].event, 'step');
    assert.equal(Object.hasOwn(events[0], 'guarded_completion'), false);
    assert.equal(receipt.providerCalls, 2);
    assert.equal(jsonl.includes(TOKEN), false);
  });
}