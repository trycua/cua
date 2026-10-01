/**
 * Content-free receipts for passive actionable-domain precedence.
 *
 * Extends trycua/cua#3963 still-open **passive native observations** and
 * kvnloo/cua#8 / trycua/cua#3904 contract vectors.
 *
 * RFC #3904 is review-only (no Driver product). This leaf attributes the
 * conservative rule: if any actionable row matches, evaluate that domain
 * only; passive participates only when actionable matches are zero.
 * Passive rows never mint action authority.
 */

export type VerificationStatus = 'satisfied' | 'unsatisfied' | 'unknown';
export type PrecedenceReason =
  | 'allowed_actionable'
  | 'allowed_passive'
  | 'multi_match'
  | 'untrusted_source'
  | 'unsupported_predicate'
  | 'incomplete_domains'
  | 'no_match'
  | 'passive_not_actionable';
export type DomainUsed = 'actionable' | 'passive' | 'none';

export type DomainRow = Readonly<{
  identity: string;
  domain: 'actionable' | 'passive';
  value?: string | null;
  enabled?: boolean | null;
  selected?: boolean | null;
  trusted?: boolean;
  supports_value?: boolean;
}>;

export type PassiveEvidence = Readonly<{
  status: VerificationStatus;
  reason: PrecedenceReason;
  domain_used: DomainUsed;
  actionable_matches: number;
  passive_matches: number;
  action_authority_minted: boolean;
  selected_identity: string | null;
}>;

export type ActionAuthorityEvidence = Readonly<{
  allowed: boolean;
  reason: PrecedenceReason;
  identity: string | null;
  action_authority_minted: boolean;
}>;

function propertyHolds(
  row: DomainRow,
  expectValue: string | null | undefined,
  expectEnabled: boolean | null | undefined
): VerificationStatus | null {
  if (expectValue != null) {
    if (row.supports_value === false || row.value == null) {
      return null;
    }
    return row.value === expectValue ? 'satisfied' : 'unsatisfied';
  }
  if (expectEnabled != null) {
    if (row.enabled == null) {
      return null;
    }
    return row.enabled === expectEnabled ? 'satisfied' : 'unsatisfied';
  }
  return 'satisfied';
}

export function evaluatePassivePrecedence(
  actionable: readonly DomainRow[],
  passive: readonly DomainRow[],
  options: {
    expect_value?: string | null;
    expect_enabled?: boolean | null;
    exists?: boolean | null;
    actionable_complete?: boolean;
    passive_complete?: boolean;
  } = {}
): PassiveEvidence {
  const actionableMatches = actionable.length;
  const passiveMatches = passive.length;
  const expectValue = options.expect_value;
  const expectEnabled = options.expect_enabled;
  const exists = options.exists;
  const actionableComplete = options.actionable_complete ?? true;
  const passiveComplete = options.passive_complete ?? true;

  const receipt = (
    status: VerificationStatus,
    reason: PrecedenceReason,
    domainUsed: DomainUsed,
    selectedIdentity: string | null = null
  ): PassiveEvidence =>
    Object.freeze({
      status,
      reason,
      domain_used: domainUsed,
      actionable_matches: actionableMatches,
      passive_matches: passiveMatches,
      action_authority_minted: false,
      selected_identity: selectedIdentity,
    });

  if (actionableMatches >= 1) {
    if (actionableMatches > 1 && (expectValue != null || expectEnabled != null)) {
      return receipt('unknown', 'multi_match', 'actionable');
    }
    const row = actionable[0];
    if (row.trusted === false) {
      return receipt('unknown', 'untrusted_source', 'actionable');
    }
    if (exists === true) {
      return receipt('satisfied', 'allowed_actionable', 'actionable', row.identity);
    }
    const hold = propertyHolds(row, expectValue, expectEnabled);
    if (hold == null) {
      return receipt('unknown', 'unsupported_predicate', 'actionable');
    }
    return receipt(
      hold,
      'allowed_actionable',
      'actionable',
      hold === 'satisfied' ? row.identity : null
    );
  }

  if (passiveMatches === 0) {
    if (actionableComplete && passiveComplete) {
      return receipt('unsatisfied', 'no_match', 'none');
    }
    return receipt('unknown', 'incomplete_domains', 'none');
  }

  if (passiveMatches > 1 && (expectValue != null || expectEnabled != null)) {
    return receipt('unknown', 'multi_match', 'passive');
  }

  const row = passive[0];
  if (row.trusted === false) {
    return receipt('unknown', 'untrusted_source', 'passive');
  }
  if (exists === true) {
    return receipt('satisfied', 'allowed_passive', 'passive', row.identity);
  }
  const hold = propertyHolds(row, expectValue, expectEnabled);
  if (hold == null) {
    return receipt('unknown', 'unsupported_predicate', 'passive');
  }
  return receipt(
    hold,
    'allowed_passive',
    'passive',
    hold === 'satisfied' ? row.identity : null
  );
}

export function explainActionAuthority(row: DomainRow): ActionAuthorityEvidence {
  if (row.domain === 'passive') {
    return Object.freeze({
      allowed: false,
      reason: 'passive_not_actionable',
      identity: null,
      action_authority_minted: false,
    });
  }
  return Object.freeze({
    allowed: true,
    reason: 'allowed_actionable',
    identity: row.identity,
    action_authority_minted: true,
  });
}