import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import process from 'node:process';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

// Native step timing fields, TypeScript runner (kvnloo/cua#75).
//
// Runs run_native.ts against fake_native_driver.py and checks the common timing fields
// against the same contract python/tests/test_native_timing.py checks for run_native.py:
// fixtures/native/timing-contract-v1.json. The driver is scripted, so nothing here measures
// Driver speed: assertions are lower bounds from the injected delays, alias equalities, and
// the nesting of phase intervals within a step.

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
type Json = Record<string, any>;
const contract: Json = JSON.parse(readFileSync(path.join(root, 'fixtures/native/timing-contract-v1.json'), 'utf8'));
const tolerance: number = contract.tolerance_ms;

function runScenario(name: string): { events: Json[]; calls: string[] } {
  const scenario = contract.scenarios[name];
  const directory = mkdtempSync(path.join(tmpdir(), 'jev-native-timing-'));
  try {
    const state = path.join(directory, 'state.json');
    const log = path.join(directory, 'events.jsonl');
    const calls = path.join(directory, 'calls.txt');
    const result = spawnSync(
      process.execPath,
      ['--import', 'tsx', 'typescript/run_native.ts', '--task', scenario.task, '--pid', String(scenario.pid),
        '--state-file', state, '--provider', 'mock', '--platform', scenario.platform, '--log', log],
      {
        cwd: root,
        encoding: 'utf8',
        timeout: 120_000,
        env: {
          ...process.env,
          CUA_DRIVER_BIN: path.join(root, 'fake_native_driver.py'),
          CUA_DRIVER_FAKE_SCENARIO: name,
          CUA_DRIVER_FAKE_STATE_FILE: state,
          CUA_DRIVER_FAKE_PID: String(scenario.pid),
          CUA_DRIVER_FAKE_DELAYS_MS: JSON.stringify(contract.delays_ms),
          CUA_DRIVER_FAKE_CALLS_FILE: calls,
        },
      }
    );
    assert.equal(result.status, 0, `run_native.ts exited ${result.status}: ${result.stderr.slice(-600)}`);
    return {
      events: readFileSync(log, 'utf8').trim().split('\n').map((line) => JSON.parse(line)),
      calls: readFileSync(calls, 'utf8').split(/\s+/).filter(Boolean),
    };
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

function checkScenario(name: string): void {
  const scenario = contract.scenarios[name];
  const delays = contract.delays_ms;
  const { events, calls } = runScenario(name);
  const steps = events.filter((event) => event.event === 'step');
  assert.equal(events.at(-1)?.outcome, 'verified');
  assert.equal(steps.length, scenario.steps);
  // Timing must not change what the runner asks the Driver to do.
  assert.deepEqual(calls, scenario.driver_calls);

  for (const step of steps) {
    for (const field of contract.common_fields as string[]) {
      assert.equal(typeof step[field], 'number', field);
      assert.ok(step[field] >= 0, field);
    }
    assert.equal(step.visual_observe_scope, contract.visual_observe_scope);
    // Legacy names are still emitted, in their original places.
    assert.equal(typeof step.decide_ms, 'number');
    assert.equal(typeof step.act_ms, 'number');
    assert.equal(typeof step.observation.observe_ms, 'number');
    // Common names map onto the native ones.
    for (const [common, legacy] of Object.entries(contract.aliases as Record<string, string>)) {
      assert.equal(step[common], step[legacy], `${common} != ${legacy}`);
    }

    const observeCalls = step.observation.reobserved ? 2 : 1;
    assert.ok(step.semantic_observe_ms + tolerance >= delays.observe * observeCalls, 'semantic_observe_ms');
    assert.ok(step.action_ms + tolerance >= delays.act, 'action_ms');
    // observe_ms spans the semantic observation plus the source/plan work in observeStep.
    assert.ok(step.observation.observe_ms + tolerance >= step.semantic_observe_ms, 'observe_ms');

    if (step.visual.status === 'ok') {
      assert.equal(step.visual_observe_ms, step.visual.parse_ms);
      assert.ok(step.visual_observe_ms + tolerance >= delays.parse, 'visual_observe_ms');
    } else {
      assert.equal(step.visual_observe_ms, 0);
    }
    assert.equal(step.visual.status, scenario.visual_status);

    const named =
      step.semantic_observe_ms + step.visual_observe_ms + step.candidate_build_ms + step.provider_decision_ms;
    assert.ok(step.decision_ms + tolerance >= named, 'decision_ms');
    assert.ok(step.total_step_ms + tolerance >= step.decision_ms + step.action_ms, 'total_step_ms');
  }
}

const skip = process.platform === 'win32' ? 'the scripted driver is a POSIX executable script' : false;

test('form task reports common fields without a visual parse', { skip }, () => {
  checkScenario('counter');
});

test('visual fallback reports parse-only visual time and both observations', { skip }, () => {
  checkScenario('canvas');
});