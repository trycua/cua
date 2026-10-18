/**
 * Every failure in this SDK is an exception with a stable `code`.
 *
 * There is exactly one rule here and it is not negotiable: **no call in this
 * SDK resolves successfully unless the remote side proved the effect
 * happened.** Cua has shipped clicks that acked in 3ms and injected nothing,
 * uploads that reported success for files that never landed, and a decoder
 * that drained every chunk and emitted nothing. An SDK is where that class of
 * bug becomes a third party's outage, so the primitives here verify and then
 * throw rather than return an optimistic value.
 */

/** Stable, matchable error codes. Adding a code is a minor release; removing or
 *  repurposing one is a breaking release. */
export type SpacesErrorCode =
  /** Credentials missing or rejected by the token endpoint. */
  | 'auth'
  /** Transport-level failure: DNS, connect, TLS, socket reset, timeout. */
  | 'transport'
  /** The control plane answered, but with a non-success status. */
  | 'http'
  /** The control plane answered with a body this SDK cannot interpret. */
  | 'protocol'
  /** The caller passed something this SDK rejects before any I/O. */
  | 'usage'
  /** A Space, thread or run id does not exist (or is no longer bound). */
  | 'not_found'
  /** A bare name matches Spaces in more than one location; the message lists
   * the ids to use (`local:<name>`, `cloud:<name>`). */
  | 'ambiguous_sandbox'
  /** An operation timed out waiting for a remote state change. */
  | 'timeout'
  /** A file transfer completed but could not be proved intact. */
  | 'verification'
  /** A shared-Space operation was refused because isolation was not acknowledged. */
  | 'isolation'
  /** A command ran inside the Space and exited non-zero. */
  | 'space_command'
  /** The host presentation channel (Cua Spaces desktop app) is not reachable. */
  | 'host_unavailable'
  /** The requested capability exists in the product but not on this path yet. */
  | 'unsupported'
  /** The Space's spacesd reports a feature this call needs as unsupported. */
  | 'capability_missing'
  /** A host-side prerequisite (app sessions, a local runtime) is missing. */
  | 'host_capability_missing'
  /** The Space has no cua-spacesd. */
  | 'spacesd_not_available'
  /** The teleport consent gate refused (or the approver declined). */
  | 'teleport_refused';

export interface SpacesErrorOptions {
  cause?: unknown;
  /** Free-form, already-redacted detail shown in the message. */
  detail?: string;
  /** HTTP status, when code is 'http'. */
  status?: number;
  /** Request target, for triage. Never carries credentials. */
  target?: string;
}

/** Trim an untrusted body for an error message without hiding it entirely. */
export function excerpt(body: string, max = 400): string {
  const flat = body.replace(/\s+/g, ' ').trim();
  return flat.length <= max ? flat : `${flat.slice(0, max)}… (${flat.length} bytes)`;
}

export class SpacesError extends Error {
  readonly code: SpacesErrorCode;
  readonly status: number | undefined;
  readonly target: string | undefined;
  readonly detail: string | undefined;

  constructor(code: SpacesErrorCode, message: string, options: SpacesErrorOptions = {}) {
    super(message, options.cause === undefined ? undefined : { cause: options.cause });
    this.name = 'SpacesError';
    this.code = code;
    this.status = options.status;
    this.target = options.target;
    this.detail = options.detail;
  }

  static auth(message: string, options?: SpacesErrorOptions): SpacesError {
    return new SpacesError('auth', message, options);
  }
  static usage(message: string, options?: SpacesErrorOptions): SpacesError {
    return new SpacesError('usage', message, options);
  }
  static protocol(message: string, options?: SpacesErrorOptions): SpacesError {
    return new SpacesError('protocol', message, options);
  }
  static timeout(message: string, options?: SpacesErrorOptions): SpacesError {
    return new SpacesError('timeout', message, options);
  }
}

/** Narrowing helper for consumers: `if (isSpacesError(e, 'verification')) …` */
export function isSpacesError(value: unknown, code?: SpacesErrorCode): value is SpacesError {
  return value instanceof SpacesError && (code === undefined || value.code === code);
}

/** Maps a `CuaError` variant tag (from the native binding) to a code. */
const CUA_TAGS: Record<string, SpacesErrorCode> = {
  InvalidArgument: 'usage',
  NotFound: 'not_found',
  AmbiguousSandbox: 'ambiguous_sandbox',
  ProviderNotConfigured: 'auth',
  Unsupported: 'unsupported',
  SpacesdNotAvailable: 'spacesd_not_available',
  Timeout: 'timeout',
  Fleet: 'http',
  // Fleet's admission refused a size over the account's limits (Fleet's
  // message names the limit): pick a smaller size.
  FleetAdmissionDenied: 'usage',
  // Out of Cua Cloud credit (the message names the billing page): add
  // credit there, or run locally.
  CloudCreditExhausted: 'usage',
  Runtime: 'space_command',
  Env: 'space_command',
  Http: 'http',
  Unauthenticated: 'auth',
  PermissionDenied: 'auth',
  Transport: 'transport',
  // No `cua daemon` answers (start Cua, or run `cua daemon start`).
  DaemonNotRunning: 'transport',
  Closed: 'transport',
  CapabilityMissing: 'capability_missing',
  HostCapabilityMissing: 'host_capability_missing',
  TeleportRefused: 'teleport_refused',
  Internal: 'protocol',
  // A pull, build or VM create would fill the disk (`cua cache prune`).
  InsufficientDisk: 'space_command',
  // A catalog image CI has not published yet (pass the ref explicitly).
  ImageNotPublished: 'not_found',
};

/**
 * Wraps anything the native SDK threw as a {@link SpacesError} with a stable
 * code (the `CuaError` variant stays reachable as `cause`). Already-wrapped
 * errors pass through.
 */
export function toSpacesError(error: unknown): SpacesError {
  if (error instanceof SpacesError) return error;
  const tag = (error as { tag?: unknown } | null)?.tag;
  const code = typeof tag === 'string' ? CUA_TAGS[tag] : undefined;
  const message = error instanceof Error ? error.message : String(error);
  return new SpacesError(code ?? 'protocol', message, { cause: error });
}

/** Runs `body`, rethrowing native errors as {@link SpacesError}. */
export async function wrapErrors<T>(body: () => Promise<T>): Promise<T> {
  try {
    return await body();
  } catch (error) {
    throw toSpacesError(error);
  }
}
