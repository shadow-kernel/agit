// Typed client for the agit server HTTP API v1 (docs/API.md). Runtime-agnostic:
// only needs a WHATWG `fetch` (Node 18+, Deno, Bun, browsers).

import { AgitError, errorFromResponse, isAbortError, isErrorBody } from "./errors.js";
import { OBJECT_STREAM_CONTENT_TYPE } from "./objects.js";
import { parseSseStream } from "./sse.js";
import type * as T from "./types.js";

export const DEFAULT_BASE_URL = "http://127.0.0.1:7878/api/v1";
export const DEFAULT_TIMEOUT_MS = 30_000;

export interface AgitClientOptions {
  /** API base including `/api/v1`. Default: `$AGIT_SERVER_URL` or `http://127.0.0.1:7878/api/v1`. */
  baseUrl?: string;
  /** Bearer token. Default: `$AGIT_TOKEN`. */
  token?: string;
  /** Acting agent id, sent as `X-Agit-Agent`. Default: `$AGIT_AGENT`. */
  agent?: string;
  /** Model name, sent as `X-Agit-Model`. Default: `$AGIT_MODEL`. */
  model?: string;
  /** Custom fetch (tests, proxies, instrumentation). Default: `globalThis.fetch`. */
  fetch?: typeof fetch;
  /** Per-request timeout for non-streaming calls; `0` disables. Default 30000. */
  timeoutMs?: number;
  /** Extra headers merged into every request. */
  headers?: Record<string, string>;
}

export interface RequestOptions {
  signal?: AbortSignal;
  /** Overrides the client timeout for this call; `0` disables. */
  timeoutMs?: number;
}

export interface StreamEventsOptions extends RequestOptions {
  /** Replay events with `seq > since` before going live. Omit to receive only new events. */
  since?: number;
  /** Reconnect (resuming from the last seen `seq`) when the connection drops. Default `true`. */
  reconnect?: boolean;
  /** Initial delay between reconnects in ms (doubles up to 30 s; honours the server's `retry:`). Default 1000. */
  retryMs?: number;
  /** Give up after this many consecutive failed connection attempts. Default: unlimited. */
  maxRetries?: number;
}

export type QueryValue = string | number | boolean | undefined | null;
export type Query = Record<string, QueryValue>;

// ---- ExpectSpec helpers -----------------------------------------------------

/** The ref must currently point at `id`. */
export function expectIs(id: T.ObjectId): T.ExpectSpec {
  return { is: id };
}
/** The ref must not exist yet (create). */
export function expectAbsent(): T.ExpectSpec {
  return { absent: true };
}
/** No precondition (last writer wins). */
export function expectAny(): T.ExpectSpec {
  return { any: true };
}

// ---- env --------------------------------------------------------------------

function readEnv(name: string): string | undefined {
  try {
    const g = globalThis as unknown as {
      process?: { env?: Record<string, string | undefined> };
      Deno?: { env?: { get(name: string): string | undefined } };
    };
    const v = g.process?.env?.[name];
    if (v !== undefined && v !== "") return v;
    const d = g.Deno?.env?.get(name);
    if (d !== undefined && d !== "") return d;
  } catch {
    /* no env access (browser, Deno without --allow-env) */
  }
  return undefined;
}

// ---- internals --------------------------------------------------------------

const enc = encodeURIComponent;
const repoPath = (repo: string): string => `/repos/${enc(repo)}`;

interface InternalRequest extends RequestOptions {
  query?: Query;
  /** JSON body, or raw bytes (sent as the object stream content type). */
  body?: unknown;
  accept?: string;
}

function linkSignals(timeoutMs: number, outer?: AbortSignal): { signal: AbortSignal | undefined; cleanup: () => void; timedOut: () => boolean } {
  // Without a timeout the caller's signal is handed to fetch as-is, so aborting it also
  // tears down a streaming body that outlives this function.
  if (timeoutMs <= 0) return { signal: outer, cleanup: () => {}, timedOut: () => false };
  const ctl = new AbortController();
  let timedOut = false;
  const onAbort = () => ctl.abort(outer?.reason);
  if (outer) {
    if (outer.aborted) onAbort();
    else outer.addEventListener("abort", onAbort, { once: true });
  }
  const timer =
    timeoutMs > 0
      ? setTimeout(() => {
          timedOut = true;
          ctl.abort();
        }, timeoutMs)
      : undefined;
  return {
    signal: ctl.signal,
    cleanup: () => {
      if (timer !== undefined) clearTimeout(timer);
      outer?.removeEventListener("abort", onAbort);
    },
    timedOut: () => timedOut,
  };
}

function sleep(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    if (signal?.aborted) return resolve();
    const t = setTimeout(done, ms);
    function done() {
      clearTimeout(t);
      signal?.removeEventListener("abort", done);
      resolve();
    }
    signal?.addEventListener("abort", done, { once: true });
  });
}

// ---- client -----------------------------------------------------------------

export class AgitClient {
  readonly baseUrl: string;
  readonly token: string | undefined;
  readonly agent: string | undefined;
  readonly model: string | undefined;
  readonly timeoutMs: number;
  private readonly fetchImpl: typeof fetch;
  private readonly extraHeaders: Record<string, string>;
  private readonly options: AgitClientOptions;

  constructor(opts: AgitClientOptions = {}) {
    this.options = { ...opts };
    this.baseUrl = (opts.baseUrl ?? readEnv("AGIT_SERVER_URL") ?? DEFAULT_BASE_URL).replace(/\/+$/, "");
    this.token = opts.token ?? readEnv("AGIT_TOKEN");
    this.agent = opts.agent ?? readEnv("AGIT_AGENT");
    this.model = opts.model ?? readEnv("AGIT_MODEL");
    this.timeoutMs = opts.timeoutMs ?? DEFAULT_TIMEOUT_MS;
    this.extraHeaders = { ...(opts.headers ?? {}) };
    const f = opts.fetch ?? (globalThis.fetch as typeof fetch | undefined);
    if (typeof f !== "function") {
      throw new AgitError({ code: "no_fetch", message: "no global fetch available; pass `fetch` in AgitClientOptions" });
    }
    this.fetchImpl = f;
  }

  /** A copy of this client acting as a different agent (and optionally model). */
  withAgent(agent: string, model?: string): AgitClient {
    return new AgitClient({ ...this.options, baseUrl: this.baseUrl, token: this.token, agent, model: model ?? this.model, fetch: this.fetchImpl });
  }

  /** Headers sent with every request (useful when calling `fetch` yourself). */
  headers(): Record<string, string> {
    const h: Record<string, string> = { ...this.extraHeaders };
    if (this.token) h["Authorization"] = `Bearer ${this.token}`;
    if (this.agent) h["X-Agit-Agent"] = this.agent;
    if (this.model) h["X-Agit-Model"] = this.model;
    return h;
  }

  /** Absolute URL for an API path and query. */
  url(path: string, query?: Query): string {
    const url = this.baseUrl + path;
    if (!query) return url;
    const params = new URLSearchParams();
    for (const [k, v] of Object.entries(query)) {
      if (v === undefined || v === null || v === "") continue;
      params.set(k, typeof v === "boolean" ? (v ? "1" : "0") : String(v));
    }
    const qs = params.toString();
    return qs ? `${url}?${qs}` : url;
  }

  // ---- HTTP core ------------------------------------------------------------

  /**
   * Perform a request and return the `Response` (errors already mapped). The timeout, when
   * set, covers connecting and the response headers only; pass `timeoutMs: 0` plus a
   * `signal` to control a streaming body.
   */
  async raw(method: string, path: string, opts: InternalRequest = {}): Promise<Response> {
    const { res, done } = await this.send(method, path, opts);
    done();
    return res;
  }

  /** Like `raw`, but the timeout stays armed until `done()` is called (after the body is read). */
  private async send(method: string, path: string, opts: InternalRequest = {}): Promise<{ res: Response; done: () => void; timedOut: () => boolean }> {
    const url = this.url(path, opts.query);
    const headers: Record<string, string> = { Accept: opts.accept ?? "application/json", ...this.headers() };
    let body: BodyInit | undefined;
    if (opts.body instanceof Uint8Array) {
      headers["Content-Type"] = OBJECT_STREAM_CONTENT_TYPE;
      body = opts.body as unknown as BodyInit;
    } else if (opts.body !== undefined) {
      headers["Content-Type"] = "application/json";
      body = JSON.stringify(opts.body);
    }
    const timeoutMs = opts.timeoutMs ?? this.timeoutMs;
    const link = linkSignals(timeoutMs, opts.signal);
    let res: Response;
    try {
      res = await this.fetchImpl(url, { method, headers, body, signal: link.signal });
    } catch (e) {
      link.cleanup();
      if (isAbortError(e)) {
        if (link.timedOut()) {
          throw new AgitError({ code: "timeout", message: `${method} ${url} timed out after ${timeoutMs} ms`, retryable: true, cause: e });
        }
        throw e;
      }
      throw new AgitError({ code: "network", message: `cannot reach ${this.baseUrl}: ${e instanceof Error ? e.message : String(e)}`, retryable: true, cause: e });
    }
    if (!res.ok) {
      link.cleanup();
      let parsed: unknown = null;
      try {
        const text = await res.text();
        parsed = text ? JSON.parse(text) : null;
      } catch {
        parsed = null;
      }
      throw errorFromResponse(res.status, parsed, res.statusText);
    }
    return { res, done: link.cleanup, timedOut: link.timedOut };
  }

  /** Read a whole body under the request's timeout, mapping a timeout to an `AgitError`. */
  private async readBody<B>(method: string, path: string, opts: InternalRequest, read: (res: Response) => Promise<B>): Promise<{ res: Response; body: B }> {
    const { res, done, timedOut } = await this.send(method, path, opts);
    try {
      return { res, body: await read(res) };
    } catch (e) {
      if (isAbortError(e) && timedOut()) {
        throw new AgitError({ code: "timeout", message: `${method} ${path} timed out while reading the response`, retryable: true, cause: e });
      }
      throw e;
    } finally {
      done();
    }
  }

  /** JSON request; empty bodies resolve to `undefined`. */
  async request<R>(method: string, path: string, opts: InternalRequest = {}): Promise<R> {
    const { res, body: text } = await this.readBody(method, path, opts, (r) => r.text());
    if (!text) return undefined as R;
    let json: unknown;
    try {
      json = JSON.parse(text);
    } catch (e) {
      throw new AgitError({ code: "invalid_response", message: `expected JSON from ${method} ${path}`, status: res.status, cause: e });
    }
    if (isErrorBody(json)) throw errorFromResponse(res.status, json, res.statusText);
    return json as R;
  }

  private async bytes(method: string, path: string, opts: InternalRequest = {}): Promise<Uint8Array> {
    const { body } = await this.readBody(method, path, { accept: "application/octet-stream, application/x-agit-objects", ...opts }, (r) => r.arrayBuffer());
    return new Uint8Array(body);
  }

  // ---- health ---------------------------------------------------------------

  health(opts?: RequestOptions): Promise<T.Health> {
    return this.request("GET", "/health", opts);
  }

  // ---- repositories ---------------------------------------------------------

  listRepos(opts?: RequestOptions): Promise<T.RepoSummary[]> {
    return this.request("GET", "/repos", opts);
  }
  createRepo(body: T.CreateRepoRequest, opts?: RequestOptions): Promise<T.RepoSummary> {
    return this.request("POST", "/repos", { ...opts, body });
  }
  getRepo(repo: string, opts?: RequestOptions): Promise<T.RepoDetail> {
    return this.request("GET", repoPath(repo), opts);
  }
  deleteRepo(repo: string, opts?: RequestOptions): Promise<T.DeleteRepoResponse> {
    return this.request("DELETE", repoPath(repo), opts);
  }

  // ---- refs -----------------------------------------------------------------

  listRefs(repo: string, prefix?: string, opts?: RequestOptions): Promise<T.RefEntry[]> {
    return this.request("GET", `${repoPath(repo)}/refs`, { ...opts, query: { prefix } });
  }
  /** Atomic compare-and-swap over all `updates`. Throws `RefConflictError` on a lost race. */
  updateRefs(repo: string, updates: T.RefUpdate[], message?: string, opts?: RequestOptions): Promise<T.RefUpdateResponse> {
    const body: T.RefUpdateRequest = message === undefined ? { updates } : { updates, message };
    return this.request("POST", `${repoPath(repo)}/refs/update`, { ...opts, body });
  }

  // ---- objects --------------------------------------------------------------

  async missingObjects(repo: string, ids: T.ObjectId[], opts?: RequestOptions): Promise<T.ObjectId[]> {
    const r = await this.request<T.MissingObjectsResponse>("POST", `${repoPath(repo)}/objects/missing`, { ...opts, body: { ids } });
    return r.missing;
  }
  /** Upload an encoded object stream (see `encodeObjectStream`). */
  uploadObjects(repo: string, stream: Uint8Array, opts?: RequestOptions): Promise<T.StoreObjectsResponse> {
    return this.request("POST", `${repoPath(repo)}/objects`, { ...opts, body: stream });
  }
  /** Raw object payload plus its kind from `X-Agit-Kind`. */
  async getObject(repo: string, id: T.ObjectId, opts?: RequestOptions): Promise<T.RawObject> {
    const { res, body } = await this.readBody("GET", `${repoPath(repo)}/objects/${enc(id)}`, { ...opts, accept: "application/octet-stream" }, (r) => r.arrayBuffer());
    const kind = (res.headers.get("x-agit-kind") ?? "blob") as T.ObjectKind;
    return { kind, payload: new Uint8Array(body) };
  }
  getObjectJson(repo: string, id: T.ObjectId, opts?: RequestOptions): Promise<T.ObjectJson> {
    return this.request("GET", `${repoPath(repo)}/objects/${enc(id)}/json`, opts);
  }
  /** Object stream of everything reachable from `wants` and not from `haves` (decode with `decodeObjectStream`). */
  fetchObjects(repo: string, wants: T.ObjectId[], haves: T.ObjectId[] = [], opts?: RequestOptions): Promise<Uint8Array> {
    return this.bytes("POST", `${repoPath(repo)}/fetch`, { ...opts, body: { wants, haves } satisfies T.FetchObjectsRequest });
  }

  // ---- browsing -------------------------------------------------------------

  listCommits(repo: string, query: T.ListCommitsQuery = {}, opts?: RequestOptions): Promise<T.CommitEntry[]> {
    return this.request("GET", `${repoPath(repo)}/commits`, { ...opts, query: { ...query } });
  }
  getCommit(repo: string, rev: string, opts?: RequestOptions): Promise<T.CommitDetail> {
    return this.request("GET", `${repoPath(repo)}/commits/${enc(rev)}`, opts);
  }
  getTree(repo: string, rev: string, path?: string, opts?: RequestOptions): Promise<T.TreeResponse> {
    return this.request("GET", `${repoPath(repo)}/tree/${enc(rev)}`, { ...opts, query: { path } });
  }
  getDiff(repo: string, from: string, to: string, opts?: RequestOptions): Promise<T.DiffResponse> {
    return this.request("GET", `${repoPath(repo)}/diff`, { ...opts, query: { from, to } });
  }
  listBranches(repo: string, opts?: RequestOptions): Promise<T.BranchEntry[]> {
    return this.request("GET", `${repoPath(repo)}/branches`, opts);
  }
  /** Repository tags (`refs/tags/*`). For knowledge tags see `listNoteTags`. */
  listTags(repo: string, opts?: RequestOptions): Promise<T.RefEntry[]> {
    return this.request("GET", `${repoPath(repo)}/tags`, opts);
  }

  // ---- events ---------------------------------------------------------------

  listEvents(repo: string, query: { since?: number; limit?: number } = {}, opts?: RequestOptions): Promise<T.Event[]> {
    return this.request("GET", `${repoPath(repo)}/events`, { ...opts, query: { ...query } });
  }

  /**
   * Live event stream (`GET /events/stream`, SSE over `fetch`). Yields events as they arrive
   * and transparently reconnects from the last seen `seq`. Stop by breaking out of the loop
   * or aborting `signal`. Non-retryable server errors (404, 403) are thrown.
   */
  async *streamEvents(repo: string, opts: StreamEventsOptions = {}): AsyncGenerator<T.Event, void, undefined> {
    const { signal, reconnect = true, maxRetries = Infinity } = opts;
    let since = opts.since;
    const baseRetry = Math.max(0, opts.retryMs ?? 1000);
    let retryMs = baseRetry;
    let failures = 0;
    for (;;) {
      if (signal?.aborted) return;
      let res: Response;
      try {
        res = await this.raw("GET", `${repoPath(repo)}/events/stream`, { query: { since }, signal, accept: "text/event-stream", timeoutMs: 0 });
      } catch (e) {
        if (signal?.aborted || isAbortError(e)) return;
        if (e instanceof AgitError && !e.retryable && e.status !== 0 && e.status < 500) throw e;
        if (!reconnect || ++failures > maxRetries) throw e;
        await sleep(retryMs, signal);
        retryMs = Math.min(retryMs * 2 || 1000, 30_000);
        continue;
      }
      if (!res.body) {
        if (!reconnect) return;
        await sleep(retryMs, signal);
        continue;
      }
      try {
        for await (const msg of parseSseStream(res.body)) {
          if (msg.retry !== undefined) retryMs = msg.retry;
          let ev: T.Event;
          try {
            ev = JSON.parse(msg.data) as T.Event;
          } catch {
            continue; // not an event frame
          }
          if (typeof ev.seq === "number") since = ev.seq;
          failures = 0;
          retryMs = msg.retry ?? baseRetry;
          yield ev;
        }
      } catch (e) {
        if (signal?.aborted || isAbortError(e)) return;
        // mid-stream network failure: fall through and reconnect
      }
      if (signal?.aborted || !reconnect) return;
      if (++failures > maxRetries) {
        throw new AgitError({ code: "stream_closed", message: `event stream for ${repo} closed after ${failures - 1} reconnect attempts`, retryable: true });
      }
      await sleep(retryMs, signal);
      retryMs = Math.min(retryMs * 2 || 1000, 30_000);
    }
  }

  // ---- leases ---------------------------------------------------------------

  listLeases(repo: string, opts?: RequestOptions): Promise<T.Lease[]> {
    return this.request("GET", `${repoPath(repo)}/leases`, opts);
  }
  /** Claim paths. Throws `LeaseConflictError` (409) when another holder has an overlapping exclusive lease. */
  claimLease(repo: string, body: T.LeaseCreateRequest, opts?: RequestOptions): Promise<T.Lease> {
    return this.request("POST", `${repoPath(repo)}/leases`, { ...opts, body });
  }
  renewLease(repo: string, id: string, ttlSecs: number, opts?: RequestOptions): Promise<T.Lease> {
    return this.request("POST", `${repoPath(repo)}/leases/${enc(id)}/renew`, { ...opts, body: { ttl_secs: ttlSecs } });
  }
  releaseLease(repo: string, id: string, opts?: RequestOptions): Promise<T.LeaseReleaseResponse> {
    return this.request("DELETE", `${repoPath(repo)}/leases/${enc(id)}`, opts);
  }
  /** Resolves when `holder` may write `paths`; throws `LeaseConflictError` otherwise. */
  checkLease(repo: string, holder: string, paths: string[], opts?: RequestOptions): Promise<T.LeaseCheckResponse> {
    return this.request("POST", `${repoPath(repo)}/leases/check`, { ...opts, body: { holder, paths } });
  }

  /** Lease operations grouped by resource (same calls as the flat methods). */
  readonly leases = {
    list: (repo: string, opts?: RequestOptions) => this.listLeases(repo, opts),
    claim: (repo: string, body: T.LeaseCreateRequest, opts?: RequestOptions) => this.claimLease(repo, body, opts),
    renew: (repo: string, id: string, ttlSecs: number, opts?: RequestOptions) => this.renewLease(repo, id, ttlSecs, opts),
    release: (repo: string, id: string, opts?: RequestOptions) => this.releaseLease(repo, id, opts),
    check: (repo: string, holder: string, paths: string[], opts?: RequestOptions) => this.checkLease(repo, holder, paths, opts),
  };

  // ---- pull requests --------------------------------------------------------

  listPulls(repo: string, state?: T.PullState | "all", opts?: RequestOptions): Promise<T.PullRequest[]> {
    return this.request("GET", `${repoPath(repo)}/pulls`, { ...opts, query: { state: state === "all" ? undefined : state } });
  }
  createPull(repo: string, body: T.PullCreateRequest, opts?: RequestOptions): Promise<T.PullRequest> {
    return this.request("POST", `${repoPath(repo)}/pulls`, { ...opts, body });
  }
  getPull(repo: string, n: number, opts?: RequestOptions): Promise<T.PullDetail> {
    return this.request("GET", `${repoPath(repo)}/pulls/${n}`, opts);
  }
  getPullDiff(repo: string, n: number, opts?: RequestOptions): Promise<T.DiffResponse> {
    return this.request("GET", `${repoPath(repo)}/pulls/${n}/diff`, opts);
  }
  updatePull(repo: string, n: number, body: T.PullUpdateRequest, opts?: RequestOptions): Promise<T.PullRequest> {
    return this.request("POST", `${repoPath(repo)}/pulls/${n}/update`, { ...opts, body });
  }
  commentPull(repo: string, n: number, body: T.CommentCreateRequest, opts?: RequestOptions): Promise<T.Comment> {
    return this.request("POST", `${repoPath(repo)}/pulls/${n}/comments`, { ...opts, body });
  }
  resolveComment(repo: string, n: number, commentId: number, opts?: RequestOptions): Promise<T.PullRequest> {
    return this.request("POST", `${repoPath(repo)}/pulls/${n}/comments/${commentId}/resolve`, opts);
  }
  reviewPull(repo: string, n: number, body: T.ReviewCreateRequest, opts?: RequestOptions): Promise<T.PullRequest> {
    return this.request("POST", `${repoPath(repo)}/pulls/${n}/reviews`, { ...opts, body: { body: "", ...body } });
  }
  setCheck(repo: string, n: number, body: T.CheckCreateRequest, opts?: RequestOptions): Promise<T.PullRequest> {
    return this.request("POST", `${repoPath(repo)}/pulls/${n}/checks`, { ...opts, body });
  }
  /** Merge on the server. Throws `MergeConflictsError` (409), `RefConflictError` (409) or `PolicyViolationError` (403). */
  mergePull(repo: string, n: number, body: T.MergeRequest = {}, opts?: RequestOptions): Promise<T.MergeResponse> {
    return this.request("POST", `${repoPath(repo)}/pulls/${n}/merge`, { ...opts, body: { strategy: "merge", ...body } });
  }
  closePull(repo: string, n: number, opts?: RequestOptions): Promise<T.PullRequest> {
    return this.request("POST", `${repoPath(repo)}/pulls/${n}/close`, opts);
  }

  /** Pull request operations grouped by resource (same calls as the flat methods). */
  readonly pulls = {
    list: (repo: string, state?: T.PullState | "all", opts?: RequestOptions) => this.listPulls(repo, state, opts),
    create: (repo: string, body: T.PullCreateRequest, opts?: RequestOptions) => this.createPull(repo, body, opts),
    get: (repo: string, n: number, opts?: RequestOptions) => this.getPull(repo, n, opts),
    diff: (repo: string, n: number, opts?: RequestOptions) => this.getPullDiff(repo, n, opts),
    update: (repo: string, n: number, body: T.PullUpdateRequest, opts?: RequestOptions) => this.updatePull(repo, n, body, opts),
    comment: (repo: string, n: number, body: T.CommentCreateRequest, opts?: RequestOptions) => this.commentPull(repo, n, body, opts),
    resolveComment: (repo: string, n: number, commentId: number, opts?: RequestOptions) => this.resolveComment(repo, n, commentId, opts),
    review: (repo: string, n: number, body: T.ReviewCreateRequest, opts?: RequestOptions) => this.reviewPull(repo, n, body, opts),
    setCheck: (repo: string, n: number, body: T.CheckCreateRequest, opts?: RequestOptions) => this.setCheck(repo, n, body, opts),
    merge: (repo: string, n: number, body?: T.MergeRequest, opts?: RequestOptions) => this.mergePull(repo, n, body, opts),
    close: (repo: string, n: number, opts?: RequestOptions) => this.closePull(repo, n, opts),
  };

  // ---- knowledge ------------------------------------------------------------

  listNotes(repo: string, query: T.ListNotesQuery = {}, opts?: RequestOptions): Promise<T.NoteSummary[]> {
    return this.request("GET", `${repoPath(repo)}/knowledge/notes`, {
      ...opts,
      query: {
        tags: query.tags && query.tags.length ? query.tags.join(",") : undefined,
        path: query.path,
        q: query.q,
        commit: query.commit,
        pull: query.pull,
        limit: query.limit,
        drafts: query.drafts ? true : undefined,
        include_superseded: query.include_superseded ? true : undefined,
      },
    });
  }
  /** Notes whose covered paths changed between their `valid_at` and `rev` (default HEAD), with the `changed_paths`. */
  listStaleNotes(repo: string, query: T.ListStaleNotesQuery = {}, opts?: RequestOptions): Promise<T.StaleNote[]> {
    return this.request("GET", `${repoPath(repo)}/knowledge/stale`, { ...opts, query: { rev: query.rev, drafts: query.drafts ? true : undefined } });
  }
  getNote(repo: string, id: string, opts?: RequestOptions): Promise<T.Note> {
    return this.request("GET", `${repoPath(repo)}/knowledge/notes/${enc(id)}`, opts);
  }
  createNote(repo: string, body: T.NoteCreateRequest, opts?: RequestOptions): Promise<T.Note> {
    return this.request("POST", `${repoPath(repo)}/knowledge/notes`, { ...opts, body });
  }
  updateNote(repo: string, id: string, body: T.NotePatchRequest, opts?: RequestOptions): Promise<T.Note> {
    return this.request("PATCH", `${repoPath(repo)}/knowledge/notes/${enc(id)}`, { ...opts, body });
  }
  /** Delete a note; deleting a draft discards it. */
  deleteNote(repo: string, id: string, opts?: RequestOptions): Promise<T.DeleteNoteResponse> {
    return this.request("DELETE", `${repoPath(repo)}/knowledge/notes/${enc(id)}`, opts);
  }
  /** Confirm the note still holds at `rev` (default HEAD): sets `valid_at`, clears `stale`, bumps `updated_ms`. */
  validateNote(repo: string, id: string, rev?: string, opts?: RequestOptions): Promise<T.Note> {
    const body: T.ValidateNoteRequest = rev === undefined ? {} : { rev };
    return this.request("POST", `${repoPath(repo)}/knowledge/notes/${enc(id)}/validate`, { ...opts, body });
  }
  /** Accept a draft (clears `draft`), applying `patch` first when given. */
  acceptNote(repo: string, id: string, patch: T.NotePatchRequest = {}, opts?: RequestOptions): Promise<T.Note> {
    return this.request("POST", `${repoPath(repo)}/knowledge/notes/${enc(id)}/accept`, { ...opts, body: patch });
  }
  /** Knowledge tags with usage counts (`GET /knowledge/tags`). */
  listNoteTags(repo: string, opts?: RequestOptions): Promise<T.TagCount[]> {
    return this.request("GET", `${repoPath(repo)}/knowledge/tags`, opts);
  }
  getTagRules(repo: string, opts?: RequestOptions): Promise<T.TagRule[]> {
    return this.request("GET", `${repoPath(repo)}/knowledge/rules`, opts);
  }
  setTagRules(repo: string, rules: T.TagRule[], opts?: RequestOptions): Promise<T.TagRule[]> {
    return this.request("PUT", `${repoPath(repo)}/knowledge/rules`, { ...opts, body: rules });
  }

  /** Knowledge operations grouped by resource (same calls as the flat methods). */
  readonly knowledge = {
    listNotes: (repo: string, query?: T.ListNotesQuery, opts?: RequestOptions) => this.listNotes(repo, query, opts),
    getNote: (repo: string, id: string, opts?: RequestOptions) => this.getNote(repo, id, opts),
    createNote: (repo: string, body: T.NoteCreateRequest, opts?: RequestOptions) => this.createNote(repo, body, opts),
    updateNote: (repo: string, id: string, body: T.NotePatchRequest, opts?: RequestOptions) => this.updateNote(repo, id, body, opts),
    deleteNote: (repo: string, id: string, opts?: RequestOptions) => this.deleteNote(repo, id, opts),
    listStale: (repo: string, query?: T.ListStaleNotesQuery, opts?: RequestOptions) => this.listStaleNotes(repo, query, opts),
    validateNote: (repo: string, id: string, rev?: string, opts?: RequestOptions) => this.validateNote(repo, id, rev, opts),
    acceptNote: (repo: string, id: string, patch?: T.NotePatchRequest, opts?: RequestOptions) => this.acceptNote(repo, id, patch, opts),
    listTags: (repo: string, opts?: RequestOptions) => this.listNoteTags(repo, opts),
    getRules: (repo: string, opts?: RequestOptions) => this.getTagRules(repo, opts),
    setRules: (repo: string, rules: T.TagRule[], opts?: RequestOptions) => this.setTagRules(repo, rules, opts),
  };

  // ---- webhooks -------------------------------------------------------------

  listWebhooks(repo: string, opts?: RequestOptions): Promise<T.Webhook[]> {
    return this.request("GET", `${repoPath(repo)}/webhooks`, opts);
  }
  /** Deliveries carry `X-Agit-Event`, `X-Agit-Delivery` and, with a secret, `X-Agit-Signature: blake3=<hex>`. */
  createWebhook(repo: string, webhook: T.WebhookCreate, opts?: RequestOptions): Promise<T.Webhook> {
    return this.request("POST", `${repoPath(repo)}/webhooks`, { ...opts, body: webhook });
  }
  deleteWebhook(repo: string, id: string, opts?: RequestOptions): Promise<{ deleted: string }> {
    return this.request("DELETE", `${repoPath(repo)}/webhooks/${encodeURIComponent(id)}`, opts);
  }
  /** Queues a synthetic `ping` delivery. */
  testWebhook(repo: string, id: string, opts?: RequestOptions): Promise<{ queued: boolean }> {
    return this.request("POST", `${repoPath(repo)}/webhooks/${encodeURIComponent(id)}/test`, opts);
  }

  // ---- policy ---------------------------------------------------------------

  getPolicy(repo: string, opts?: RequestOptions): Promise<T.Policy> {
    return this.request("GET", `${repoPath(repo)}/policy`, opts);
  }
  setPolicy(repo: string, policy: T.Policy, opts?: RequestOptions): Promise<T.Policy> {
    return this.request("PUT", `${repoPath(repo)}/policy`, { ...opts, body: policy });
  }

  // ---- signing keys ---------------------------------------------------------

  /** Public signing keys registered for agents in this repository. */
  listKeys(repo: string, opts?: RequestOptions): Promise<T.KeyEntry[]> {
    return this.request("GET", `${repoPath(repo)}/keys`, opts);
  }
  /**
   * Trust an Ed25519 public key (hex, as printed by `agit key generate`) for an agent. Registering
   * the same key for the same agent again returns the existing entry; a key already registered for
   * another agent is a 409 `already_exists`.
   */
  registerKey(repo: string, key: T.KeyCreate, opts?: RequestOptions): Promise<T.KeyEntry> {
    return this.request("POST", `${repoPath(repo)}/keys`, { ...opts, body: key });
  }
  /** Revoke a key by `key_id`. */
  deleteKey(repo: string, keyId: string, opts?: RequestOptions): Promise<T.DeleteKeyResponse> {
    return this.request("DELETE", `${repoPath(repo)}/keys/${enc(keyId)}`, opts);
  }

  /** Signing key operations grouped by resource (same calls as the flat methods). */
  readonly keys = {
    list: (repo: string, opts?: RequestOptions) => this.listKeys(repo, opts),
    register: (repo: string, key: T.KeyCreate, opts?: RequestOptions) => this.registerKey(repo, key, opts),
    delete: (repo: string, keyId: string, opts?: RequestOptions) => this.deleteKey(repo, keyId, opts),
  };

  // ---- context & activity ---------------------------------------------------

  /**
   * One call returning what an agent should read before touching `path`: notes (fresh first, then
   * stale), suggested tags, recent commits, leases, open PRs and the agents active on it. Pass a
   * number as `query` for just `limit`, or a `ContextQuery` to also request file texts.
   */
  getContext(repo: string, path: string, query?: number | T.ContextQuery, opts?: RequestOptions): Promise<T.ContextResponse> {
    const q: T.ContextQuery = typeof query === "number" ? { limit: query } : (query ?? {});
    return this.request("GET", `${repoPath(repo)}/context`, {
      ...opts,
      query: { path, limit: q.limit, include: q.include_files ? "files" : undefined, budget: q.include_files ? q.budget : undefined },
    });
  }
  /** `getContext` with `include=files`: the context plus the file at `path` and its siblings, kept within `budget` approximate tokens (default 6000). */
  getContextPack(repo: string, path: string, budget?: number, opts?: RequestOptions): Promise<T.ContextResponse> {
    return this.getContext(repo, path, { include_files: true, budget }, opts);
  }
  listAgents(repo: string, opts?: RequestOptions): Promise<T.AgentActivity[]> {
    return this.request("GET", `${repoPath(repo)}/agents`, opts);
  }
}
