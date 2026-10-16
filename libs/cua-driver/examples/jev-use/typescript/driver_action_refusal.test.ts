import assert from 'node:assert/strict';
import test from 'node:test';
import { Driver, DriverToolError } from './run.js';
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
