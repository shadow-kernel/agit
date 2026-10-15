// Error model: every server error is `{ok:false, error:{code, message, retryable, details?}}`.
// `AgitError` carries that envelope; subclasses are selected by `code` so callers can
// `instanceof` the cases they handle and fall back to `code` for the rest.

import type { ApiErrorBody, LeaseConflictDetails, MergeConflictsDetails, RefConflictDetails } from "./types.js";

export interface AgitErrorInit {
  code: string;
  message: string;
  /** HTTP status; 0 for network/timeout failures that never reached the server. */
  status?: number;
  retryable?: boolean;
  details?: unknown;
  cause?: unknown;
}

export class AgitError extends Error {
  readonly code: string;
  readonly status: number;
  readonly retryable: boolean;
  readonly details: unknown;

  constructor(init: AgitErrorInit) {
    super(init.message, init.cause !== undefined ? { cause: init.cause } : undefined);
    this.name = "AgitError";
    this.code = init.code;
    this.status = init.status ?? 0;
    this.retryable = init.retryable ?? false;
    this.details = init.details;
  }

  /** `true` when the error was produced by the SDK itself (network, timeout, abort), not by the server. */
  get isTransport(): boolean {
    return this.status === 0;
  }
}

/** 409 `ref_conflict`: someone moved the ref your compare-and-swap expected. Re-read and retry. */
export class RefConflictError extends AgitError {
  declare readonly details: RefConflictDetails | undefined;
  constructor(init: AgitErrorInit) {
    super({ retryable: true, ...init });
    this.name = "RefConflictError";
  }
}

/** 409 `lease_conflict`: another agent holds an exclusive lease on the path. */
export class LeaseConflictError extends AgitError {
  declare readonly details: LeaseConflictDetails | undefined;
  constructor(init: AgitErrorInit) {
    super(init);
    this.name = "LeaseConflictError";
  }
}

/** 409 `merge_conflicts`: the three-way merge needs a decision. */
export class MergeConflictsError extends AgitError {
  declare readonly details: MergeConflictsDetails | undefined;
  constructor(init: AgitErrorInit) {
    super(init);
    this.name = "MergeConflictsError";
  }
}

/** 403 `policy_violation`: the repository policy refused the change (missing intent, approvals, protected branch...). */
export class PolicyViolationError extends AgitError {
  constructor(init: AgitErrorInit) {
    super(init);
    this.name = "PolicyViolationError";
  }
}

/** 404 `not_found` and its specialisations (`ref_not_found`, `unknown_revision`, `lease_not_found`, ...) plus 412 `object_not_found`. */
export class NotFoundError extends AgitError {
  constructor(init: AgitErrorInit) {
    super(init);
    this.name = "NotFoundError";
  }
}

const NOT_FOUND_CODES = new Set([
  "not_found",
  "ref_not_found",
  "object_not_found",
  "unknown_revision",
  "lease_not_found",
  "workspace_not_found",
  "path_not_tracked",
  "not_a_repository",
]);

/** Build the right `AgitError` subclass for an error code. */
export function createError(init: AgitErrorInit): AgitError {
  switch (init.code) {
    case "ref_conflict":
      return new RefConflictError(init);
    case "lease_conflict":
      return new LeaseConflictError(init);
    case "merge_conflicts":
      return new MergeConflictsError(init);
    case "policy_violation":
      return new PolicyViolationError(init);
    default:
      return NOT_FOUND_CODES.has(init.code) ? new NotFoundError(init) : new AgitError(init);
  }
}

/** Map an HTTP response (status + parsed body, if any) to an `AgitError`. */
export function errorFromResponse(status: number, body: unknown, statusText?: string): AgitError {
  const env = isErrorBody(body) ? body.error : undefined;
  return createError({
    status,
    code: env?.code ?? `http_${status}`,
    message: env?.message ?? (statusText ? `${status} ${statusText}` : `request failed with status ${status}`),
    retryable: env?.retryable ?? (status === 429 || status === 503 || status === 502 || status === 504),
    details: env?.details,
  });
}

export function isErrorBody(body: unknown): body is ApiErrorBody {
  if (!body || typeof body !== "object") return false;
  const b = body as Record<string, unknown>;
  if (b.ok !== false || !b.error || typeof b.error !== "object") return false;
  const e = b.error as Record<string, unknown>;
  return typeof e.code === "string" && typeof e.message === "string";
}

export function isAgitError(err: unknown): err is AgitError {
  return err instanceof AgitError;
}

/**
 * `true` when retrying the same call makes sense: the server said so (`retryable`),
 * or the failure was a transport problem (network, timeout) that never reached it.
 */
export function isRetryable(err: unknown): boolean {
  if (err instanceof AgitError) return err.retryable;
  return false;
}

/** `true` for `AbortError`s raised by `fetch` when an `AbortSignal` fires. */
export function isAbortError(err: unknown): boolean {
  return err instanceof Error && err.name === "AbortError";
}
