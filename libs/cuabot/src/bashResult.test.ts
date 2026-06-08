import assert from 'node:assert/strict';
import test from 'node:test';
import {
  bashResultFromBackgroundLaunch,
  bashResultFromExecError,
  buildContainerScript,
  buildDockerExecScriptCommand,
  exitCodeForBashResult,
} from './bashResult.js';

test('buildContainerScript sets GUI user environment', () => {
  const script = buildContainerScript('firefox');

  assert.match(script, /export DISPLAY=:100/);
  assert.match(script, /export USER=user/);
  assert.match(script, /export LOGNAME=user/);
  assert.match(script, /export HOME=\/home\/user/);
  assert.match(script, /export XDG_RUNTIME_DIR=\/tmp\/runtime-user/);
  assert.match(script, /export XAUTHORITY=\/home\/user\/\.Xauthority/);
});

test('bashResultFromExecError preserves exit code and failure status', () => {
  const result = bashResultFromExecError({ code: 42, stdout: 'out', stderr: 'err' });

  assert.equal(result.success, false);
  assert.equal(result.exit_code, 42);
  assert.equal(result.stdout, 'out');
  assert.equal(result.stderr, 'err');
});

test('bashResultFromExecError marks timeout failures', () => {
  const result = bashResultFromExecError({ code: 124, stdout: 'partial' });

  assert.equal(result.success, false);
  assert.equal(result.exit_code, 124);
  assert.equal(result.timed_out, true);
});

test('bashResultFromExecError does not infer timeout from wrapper command text', () => {
  const result = bashResultFromExecError({ code: 127, message: 'Command failed: timeout 10s nope' });

  assert.equal(result.success, false);
  assert.equal(result.exit_code, 127);
  assert.equal(result.timed_out, undefined);
});

test('exitCodeForBashResult maps structured results to local CLI status', () => {
  assert.equal(exitCodeForBashResult({ success: true, exit_code: 0 }), 0);
  assert.equal(exitCodeForBashResult({ success: false, exit_code: 42 }), 42);
  assert.equal(exitCodeForBashResult({ success: false, exit_code: null, timed_out: true }), 124);
  assert.equal(exitCodeForBashResult({ success: false, exit_code: null }), 1);
});

test('bashResultFromBackgroundLaunch advertises pid only when process is running', () => {
  const result = bashResultFromBackgroundLaunch('123\nOUTCOME:RUNNING\n', '');

  assert.equal(result.success, true);
  assert.equal(result.exit_code, 0);
  assert.equal(result.pid, 123);
});

test('bashResultFromBackgroundLaunch reports fast background exit without stale pid', () => {
  const result = bashResultFromBackgroundLaunch('123\nOUTCOME:EXIT:7\n', 'failure log\n');

  assert.equal(result.success, false);
  assert.equal(result.exit_code, 7);
  assert.equal(result.pid, undefined);
  assert.equal(result.stderr, 'failure log\n');
});

test('buildDockerExecScriptCommand omits shell timeout for non-positive timeouts', () => {
  assert.equal(
    buildDockerExecScriptCommand('container', '/tmp/script.sh', 0),
    'docker exec container /tmp/script.sh'
  );
});

test('buildDockerExecScriptCommand wraps positive timeouts with shell timeout', () => {
  assert.equal(
    buildDockerExecScriptCommand('container', '/tmp/script.sh', 1500),
    'docker exec container timeout 1.5s /tmp/script.sh'
  );
});
