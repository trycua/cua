import assert from 'node:assert/strict';
import test from 'node:test';

import {
  evaluatePassivePrecedence,
  explainActionAuthority,
  type DomainRow,
} from './passive_actionable_domain.js';

function actionable(
  identity = 'btn-eq',
  extras: Partial<DomainRow> = {}
): DomainRow {
  return Object.freeze({ identity, domain: 'actionable', ...extras });
}

function passive(
  identity = 'calc-result',
  extras: Partial<DomainRow> = {}
): DomainRow {
  return Object.freeze({
    identity,
    domain: 'passive',
    value: '42',
    ...extras,
  });
}

test('actionable unique ignores passive siblings', () => {
  const evidence = evaluatePassivePrecedence(
    [actionable('btn-eq', { value: '=' })],
    [passive('calc-result', { value: '42' }), passive('calc-expr', { value: '6x7' })],
    { expect_value: '=' }
  );
  assert.equal(evidence.status, 'satisfied');
  assert.equal(evidence.reason, 'allowed_actionable');
  assert.equal(evidence.domain_used, 'actionable');
  assert.equal(evidence.actionable_matches, 1);
  assert.equal(evidence.passive_matches, 2);
  assert.equal(evidence.action_authority_minted, false);
  assert.equal(evidence.selected_identity, 'btn-eq');
});

test('passive-only unique property satisfies', () => {
  const evidence = evaluatePassivePrecedence([], [passive()], {
    expect_value: '42',
  });
  assert.equal(evidence.status, 'satisfied');
  assert.equal(evidence.reason, 'allowed_passive');
  assert.equal(evidence.domain_used, 'passive');
  assert.equal(evidence.action_authority_minted, false);
  assert.equal(evidence.selected_identity, 'calc-result');
});

test('passive multi_match property is unknown', () => {
  const evidence = evaluatePassivePrecedence(
    [],
    [passive('a'), passive('b')],
    { expect_value: '42' }
  );
  assert.equal(evidence.status, 'unknown');
  assert.equal(evidence.reason, 'multi_match');
  assert.equal(evidence.domain_used, 'passive');
  assert.equal(evidence.selected_identity, null);
});

test('untrusted passive-only is unknown', () => {
  const evidence = evaluatePassivePrecedence(
    [],
    [passive('calc-result', { trusted: false })],
    { expect_value: '42' }
  );
  assert.equal(evidence.status, 'unknown');
  assert.equal(evidence.reason, 'untrusted_source');
});

test('passive never mints action authority', () => {
  const refused = explainActionAuthority(passive());
  const allowed = explainActionAuthority(actionable());
  assert.equal(refused.allowed, false);
  assert.equal(refused.reason, 'passive_not_actionable');
  assert.equal(refused.action_authority_minted, false);
  assert.equal(allowed.allowed, true);
  assert.equal(allowed.action_authority_minted, true);
  assert.equal(allowed.identity, 'btn-eq');
});

test('receipt is content-free', () => {
  const evidence = evaluatePassivePrecedence(
    [actionable('btn-eq', { value: '=' })],
    [passive('calc-result', { value: 'secret-token' })],
    { expect_value: '=' }
  );
  const text = JSON.stringify(evidence).toLowerCase();
  for (const forbidden of [
    'secret-token',
    'password',
    'authorization',
    'image_base64',
  ]) {
    assert.equal(text.includes(forbidden), false);
  }
  assert.equal(evidence.action_authority_minted, false);
});