import assert from 'node:assert/strict';
import test from 'node:test';
import { Driver, DriverToolError, backgroundRefusalCode } from './run.js';
function make(flag: boolean | undefined, structuredContent: unknown) {
  const calls: unknown[] = [];
  const driver = new Driver({callTool: async (args: unknown) => { calls.push(args); return {isError: flag, structuredContent, content: []}; }} as unknown as ConstructorParameters<typeof Driver>[0], 'owned');
  return {driver, calls};
}
const refusals: [string, boolean | undefined, unknown, string?, string?][] = [
  ['effect refused without error flag', undefined, {effect:'refused',code:'browser_ref_stale'}, 'browser_ref_stale'],
  ['effect refused with false flag', false, {effect:'refused',code:'browser_binding_stale'}, 'browser_binding_stale'],
  ['effect refused without optional metadata', false, {effect:'refused'}],
  ['effect refused preserves escalation', false, {effect:'refused',code:'background_unavailable',escalation:{recommended:'foreground'}}, 'background_unavailable', 'foreground'],
  ['effect refused nested code preserves escalation', false, {effect:'refused',refusal:{code:'background_unsupported'},escalation:{recommended:'foreground'}}, 'background_unsupported', 'foreground'],
  ['effect refused ignores non-string metadata', false, {effect:'refused',code:17,escalation:{recommended:['foreground']}}],
  ['effect refused wins over legacy ok', false, {effect:'refused',status:'ok',code:'permission_denied'}, 'permission_denied'],
  ['MCP error still raises', true, {code:'invalid_arguments'}, 'invalid_arguments'],
  ['MCP error without structured content', true, undefined],
  ['legacy refusal still raises', false, {refusal:{code:'denied'}}, 'denied'],
  ['legacy status still raises', false, {status:'refused'}],
];
for (const [name, flag, data, code, recommended] of refusals) test(name, async () => {
  const {driver, calls} = make(flag, data);
  await assert.rejects(driver.call('browser_click',{session:'foreign',ref:'p1:0'}), error => {
    assert.ok(error instanceof DriverToolError);
    assert.equal(error.code,code); assert.equal(error.recommendedDelivery,recommended); return true;
  });
  assert.deepEqual(calls,[{name:'browser_click', arguments:{session:'owned',ref:'p1:0'}}]);
});
for (const [name,data] of [
  ['confirmed returned unchanged',{effect:'confirmed',result:{count:1}}],
  ['unverifiable is not refusal or retry permission',{effect:'unverifiable',escalation:{recommended:'verify_state'}}],
  ['observation returned unchanged',{windows:[]}],
] as const) test(name,async () => {
  const {driver,calls}=make(false,data);
  assert.equal(await driver.call('browser_click',{}),data); assert.equal(calls.length,1);
});
test('missing structured result still raises',async () => {
  const {driver}=make(false,undefined);
  await assert.rejects(driver.call('list_windows',{}),/no structured result/);
});

async function errorFor(data: unknown, flag = false): Promise<DriverToolError> {
  const { driver, calls } = make(flag, data);
  let caught: DriverToolError | undefined;
  await assert.rejects(driver.call('click', { delivery_mode: 'background' }), error => {
    assert.ok(error instanceof DriverToolError);
    caught = error;
    return true;
  });
  assert.equal(calls.length, 1, 'normalization must not dispatch again');
  assert.ok(caught);
  return caught;
}
function recoveryCandidate(tool = 'click', delivery = 'background') {
  return { tool, arguments: { delivery_mode: delivery } } as Parameters<typeof backgroundRefusalCode>[0];
}

test('status refusal preserves top-level code and escalation', async () => {
  const error = await errorFor({ status: 'refused', code: 'background_unavailable', escalation: { recommended: 'foreground' } });
  assert.equal(error.code, 'background_unavailable');
  assert.equal(error.recommendedDelivery, 'foreground');
  assert.match(error.message, /click refused:/);
});
test('status refusal preserves nested code and escalation', async () => {
  const error = await errorFor({ status: 'refused', refusal: { code: 'unsupported_delivery' }, escalation: { recommended: 'foreground' } });
  assert.equal(error.code, 'unsupported_delivery');
  assert.equal(error.recommendedDelivery, 'foreground');
});
test('nested refusal preserves escalation without status', async () => {
  const error = await errorFor({ refusal: { code: 'unsupported_delivery' }, escalation: { recommended: 'foreground' } });
  assert.equal(error.code, 'unsupported_delivery');
  assert.equal(error.recommendedDelivery, 'foreground');
});
test('legacy refusal prefers valid top-level code', async () => {
  const error = await errorFor({ refusal: { code: 'nested_reason' }, code: 'permission_denied' });
  assert.equal(error.code, 'permission_denied');
  assert.equal(error.recommendedDelivery, undefined);
});
test('status refusal can recommend foreground without code', async () => {
  const error = await errorFor({ status: 'refused', escalation: { recommended: 'foreground' } });
  assert.equal(error.code, undefined);
  assert.equal(error.recommendedDelivery, 'foreground');
});
test('legacy non-object refusal keeps valid metadata', async () => {
  const error = await errorFor({ refusal: 'unsupported', code: 'unsupported_delivery', escalation: { recommended: 'foreground' } });
  assert.equal(error.code, 'unsupported_delivery');
  assert.equal(error.recommendedDelivery, 'foreground');
});
test('invalid top-level code uses nested code on every error path', async () => {
  for (const [flag, marker] of [[true, {}], [false, { effect: 'refused' }], [false, { status: 'refused' }]] as const) {
    const error = await errorFor({ ...marker, code: 17, refusal: { code: 'background_unavailable' } }, flag);
    assert.equal(error.code, 'background_unavailable');
  }
});
test('empty top-level code falls back to nested code', async () => {
  const error = await errorFor({ status: 'refused', code: '', refusal: { code: 'unsupported_delivery' }, escalation: { recommended: 'foreground' } });
  assert.equal(error.code, 'unsupported_delivery');
  assert.equal(error.recommendedDelivery, 'foreground');
});
test('malformed legacy metadata is not used', async () => {
  const error = await errorFor({ status: 'refused', code: 17, refusal: { code: [] }, escalation: { recommended: ['foreground'] } });
  assert.equal(error.code, undefined);
  assert.equal(error.recommendedDelivery, undefined);
});
test('legacy metadata reaches existing background recovery', async () => {
  const error = await errorFor({ status: 'refused', code: 'unsupported_delivery', escalation: { recommended: 'foreground' } });
  assert.equal(backgroundRefusalCode(recoveryCandidate(), error), 'unsupported_delivery');
});
test('recommendation without code keeps existing recovery reason', async () => {
  const error = await errorFor({ status: 'refused', escalation: { recommended: 'foreground' } });
  assert.equal(backgroundRefusalCode(recoveryCandidate(), error), 'foreground_recommended');
});
test('recovery stays limited to background click', async () => {
  const error = await errorFor({ status: 'refused', code: 'background_unavailable', escalation: { recommended: 'foreground' } });
  for (const [tool, delivery] of [['click', 'foreground'], ['type_text', 'background'], ['browser_click', 'background']]) {
    assert.equal(backgroundRefusalCode(recoveryCandidate(tool, delivery), error), undefined);
  }
});
test('permission denial is not foreground permission', async () => {
  const error = await errorFor({ status: 'refused', code: 'permission_denied' });
  assert.equal(error.code, 'permission_denied');
  assert.equal(backgroundRefusalCode(recoveryCandidate(), error), undefined);
});
test('recommendation alone does not turn observation into refusal', async () => {
  const data = { windows: [], escalation: { recommended: 'foreground' } };
  const { driver } = make(false, data);
  assert.equal(await driver.call('list_windows', {}), data);
});
