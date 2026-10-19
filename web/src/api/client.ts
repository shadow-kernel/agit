// Typed fetch wrapper for the agit server HTTP API v1 (docs/API.md).
//
// `api` is a stable facade. At startup `initApi()` probes `GET /health`; when
// the probe fails, or when VITE_AGIT_MOCK=1, the facade is backed by the
// in-memory mock implementation in ./mock.ts instead of the network.

import type * as T from "./types";
import { loadSettings } from "./settings";

export class ApiError extends Error {
  code: string;
  retryable: boolean;
  status: number;
  details?: unknown;
  constructor(status: number, code: string, message: string, retryable = false, details?: unknown) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
    this.retryable = retryable;
    this.details = details;
  }
}

export type Query = Record<string, string | number | boolean | undefined | null>;

/** Everything the UI can call. Mirrors docs/API.md one to one. */
export interface Api {
  health(): Promise<T.Health>;
  // Auth
  whoami(): Promise<T.WhoAmI>;
  tokens(): Promise<T.TokenInfo[]>;
  createToken(body: T.TokenCreateRequest): Promise<T.TokenCreated>;
  deleteToken(id: string): Promise<unknown>;
  /** SSH keys allowed to sign in (admin). */
  authorizedKeys(): Promise<T.AuthorizedKey[]>;
  addAuthorizedKey(body: T.AuthorizedKeyCreateRequest): Promise<T.AuthorizedKey>;
  deleteAuthorizedKey(id: string): Promise<{ deleted: string; revoked_sessions: number }>;
  /** SSH sign-in: a nonce to sign, then a session token (no token needed for either call). */
  challenge(): Promise<T.Challenge>;
  createSession(body: T.SessionCreateRequest): Promise<T.TokenCreated>;
  /** Revoke the token this client is using. */
  logout(): Promise<{ revoked: string }>;
  // Repositories
  repos(): Promise<T.RepoSummary[]>;
  createRepo(body: { name: string; default_branch?: string }): Promise<T.RepoSummary>;
  repo(repo: string): Promise<T.RepoDetail>;
  deleteRepo(repo: string): Promise<{ deleted: true }>;
  // Refs
  refs(repo: string, prefix?: string): Promise<T.Ref[]>;
  updateRefs(repo: string, body: T.RefUpdateRequest): Promise<unknown>;
  // Objects
  objectJson(repo: string, id: string): Promise<T.ObjectJson>;
  // Browsing
  commits(repo: string, q?: { rev?: string; limit?: number; agent?: string; task?: string; path?: string }): Promise<T.CommitListItem[]>;
  commit(repo: string, rev: string): Promise<T.CommitDetail>;
  tree(repo: string, rev: string, path?: string, signal?: AbortSignal): Promise<T.TreeResponse>;
  diff(repo: string, from: string, to: string): Promise<T.Diff>;
  blame(repo: string, rev: string, path: string, maxCommits?: number, signal?: AbortSignal): Promise<T.Blame>;
  branches(repo: string): Promise<T.Branch[]>;
  tags(repo: string): Promise<T.Tag[]>;
  // Events
  events(repo: string, since?: number, limit?: number): Promise<T.Event[]>;
  /** Live stream. Returns an unsubscribe function. */
  streamEvents(repo: string, since: number, onEvent: (e: T.Event) => void, onStatus?: (s: StreamStatus) => void): () => void;
  // Leases
  leases(repo: string): Promise<T.Lease[]>;
  createLease(repo: string, body: T.LeaseCreateRequest): Promise<T.Lease>;
  renewLease(repo: string, id: string, ttl_secs: number): Promise<T.Lease>;
  releaseLease(repo: string, id: string): Promise<unknown>;
  checkLease(repo: string, holder: string, paths: string[]): Promise<{ allowed: true }>;
  // Pull requests
  pulls(repo: string, state?: T.PullState | "all"): Promise<T.Pull[]>;
  createPull(repo: string, body: T.PullCreateRequest): Promise<T.Pull>;
  pull(repo: string, n: number): Promise<T.PullDetail>;
  pullDiff(repo: string, n: number): Promise<T.Diff>;
  addPullComment(repo: string, n: number, body: T.PullCommentRequest): Promise<T.PullComment>;
  resolvePullComment(repo: string, n: number, id: number): Promise<unknown>;
  reviewPull(repo: string, n: number, body: T.PullReviewRequest): Promise<T.Pull>;
  addPullCheck(repo: string, n: number, body: T.PullCheckRequest): Promise<T.Pull>;
  mergePull(repo: string, n: number, body: T.PullMergeRequest): Promise<T.PullMergeResponse>;
  closePull(repo: string, n: number): Promise<T.Pull>;
  updatePull(repo: string, n: number, body: T.PullUpdateRequest): Promise<T.Pull>;
  // Knowledge
  notes(repo: string, q?: T.NoteListQuery): Promise<T.NoteSummary[]>;
  note(repo: string, id: string): Promise<T.Note>;
  createNote(repo: string, body: T.NoteCreateRequest): Promise<T.Note>;
  patchNote(repo: string, id: string, body: T.NotePatchRequest): Promise<T.Note>;
  deleteNote(repo: string, id: string): Promise<unknown>;
  /** `POST .../validate`: sets `valid_at` to `rev` (default HEAD). */
  validateNote(repo: string, id: string, rev?: string): Promise<T.Note>;
  /** `POST .../accept`: clears `draft`, applying `patch` first when given. */
  acceptNote(repo: string, id: string, patch?: T.NotePatchRequest): Promise<T.Note>;
  staleNotes(repo: string, q?: { rev?: string; drafts?: boolean }): Promise<T.StaleNote[]>;
  tagCounts(repo: string): Promise<T.TagCount[]>;
  rules(repo: string): Promise<T.TagRule[]>;
  putRules(repo: string, rules: T.TagRule[]): Promise<T.TagRule[]>;
  context(repo: string, path: string, q?: T.ContextQuery): Promise<T.Context>;
  // Policy
  policy(repo: string): Promise<T.Policy>;
  putPolicy(repo: string, policy: T.Policy): Promise<T.Policy>;
  // Signing keys
  keys(repo: string): Promise<T.KeyEntry[]>;
  addKey(repo: string, body: T.KeyCreateRequest): Promise<T.KeyEntry>;
  deleteKey(repo: string, keyId: string): Promise<unknown>;
  // Webhooks
  webhooks(repo: string): Promise<T.Webhook[]>;
  createWebhook(repo: string, body: T.WebhookCreateRequest): Promise<T.Webhook>;
  deleteWebhook(repo: string, id: string): Promise<unknown>;
  testWebhook(repo: string, id: string): Promise<{ queued: true }>;
  // Activity
  agents(repo: string): Promise<T.AgentActivity[]>;
}

export type StreamStatus = "connecting" | "sse" | "polling" | "error";

// ---- Error listeners (toasts) ----------------------------------------------

type ErrorListener = (err: ApiError) => void;
const errorListeners = new Set<ErrorListener>();
export function onApiError(l: ErrorListener) {
  errorListeners.add(l);
  return () => {
    errorListeners.delete(l);
  };
}
export function reportError(err: unknown) {
  const e = err instanceof ApiError ? err : new ApiError(0, "network", err instanceof Error ? err.message : String(err), true);
  if (isAuthError(e)) return; // already surfaced by the global sign-in prompt
  errorListeners.forEach((l) => l(e));
}

// ---- Auth denial listeners (global "sign in" prompt) ------------------------

export const isAuthError = (e: ApiError) => e.status === 401 || e.status === 403;
const authListeners = new Set<ErrorListener>();
/** Called for every 401/403 the HTTP layer (or the mock) produces (`forbidden`, `policy_violation`, ...). */
export function onAuthDenied(l: ErrorListener) {
  authListeners.add(l);
  return () => {
    authListeners.delete(l);
  };
}
export function notifyAuthDenied(err: ApiError) {
  authListeners.forEach((l) => l(err));
}

// ---- HTTP core ---------------------------------------------------------------

function buildUrl(base: string, path: string, query?: Query) {
  const url = base.replace(/\/+$/, "") + path;
  if (!query) return url;
  const params = new URLSearchParams();
  for (const [k, v] of Object.entries(query)) {
    if (v === undefined || v === null || v === "") continue;
    params.set(k, String(v));
  }
  const qs = params.toString();
  return qs ? `${url}?${qs}` : url;
}

export function authHeaders(): Record<string, string> {
  const s = loadSettings();
  const h: Record<string, string> = {};
  if (s.token) h.Authorization = `Bearer ${s.token}`;
  if (s.agent) h["X-Agit-Agent"] = s.agent;
  return h;
}

async function request<R>(method: string, path: string, opts: { query?: Query; body?: unknown; signal?: AbortSignal } = {}): Promise<R> {
  const s = loadSettings();
  const url = buildUrl(s.baseUrl, path, opts.query);
  const headers: Record<string, string> = { Accept: "application/json", ...authHeaders() };
  if (opts.body !== undefined) headers["Content-Type"] = "application/json";
  let res: Response;
  try {
    res = await fetch(url, { method, headers, body: opts.body === undefined ? undefined : JSON.stringify(opts.body), signal: opts.signal });
  } catch (e) {
    if (e instanceof DOMException && e.name === "AbortError") throw e;
    throw new ApiError(0, "network", `Cannot reach ${s.baseUrl}`, true);
  }
  const text = await res.text();
  let json: unknown = null;
  if (text) {
    try {
      json = JSON.parse(text);
    } catch {
      json = null;
    }
  }
  if (!res.ok || (json && typeof json === "object" && (json as T.ApiErrorBody).ok === false)) {
    const env = json as T.ApiErrorBody | null;
    const err = env?.error;
    const apiErr = new ApiError(res.status, err?.code ?? `http_${res.status}`, err?.message ?? res.statusText ?? "Request failed", err?.retryable ?? false, err?.details);
    if (isAuthError(apiErr)) notifyAuthDenied(apiErr);
    throw apiErr;
  }
  return json as R;
}

const enc = encodeURIComponent;
const r = (repo: string) => `/repos/${enc(repo)}`;

export const liveApi: Api = {
  health: () => request("GET", "/health"),
  whoami: () => request("GET", "/auth/whoami"),
  tokens: () => request("GET", "/auth/tokens"),
  createToken: (body) => request("POST", "/auth/tokens", { body }),
  deleteToken: (id) => request("DELETE", `/auth/tokens/${enc(id)}`),
  authorizedKeys: () => request("GET", "/auth/keys"),
  addAuthorizedKey: (body) => request("POST", "/auth/keys", { body }),
  deleteAuthorizedKey: (id) => request("DELETE", `/auth/keys/${enc(id)}`),
  challenge: () => request("POST", "/auth/challenge"),
  createSession: (body) => request("POST", "/auth/session", { body }),
  logout: () => request("POST", "/auth/logout"),
  repos: () => request("GET", "/repos"),
  createRepo: (body) => request("POST", "/repos", { body }),
  repo: (repo) => request("GET", r(repo)),
  deleteRepo: (repo) => request("DELETE", r(repo)),
  refs: (repo, prefix) => request("GET", `${r(repo)}/refs`, { query: { prefix } }),
  updateRefs: (repo, body) => request("POST", `${r(repo)}/refs/update`, { body }),
  objectJson: (repo, id) => request("GET", `${r(repo)}/objects/${enc(id)}/json`),
  commits: (repo, q = {}) => request("GET", `${r(repo)}/commits`, { query: q }),
  commit: (repo, rev) => request("GET", `${r(repo)}/commits/${enc(rev)}`),
  tree: (repo, rev, path, signal) => request("GET", `${r(repo)}/tree/${enc(rev)}`, { query: { path }, signal }),
  diff: (repo, from, to) => request("GET", `${r(repo)}/diff`, { query: { from, to } }),
  blame: (repo, rev, path, maxCommits, signal) => request("GET", `${r(repo)}/blame/${enc(rev)}`, { query: { path, max_commits: maxCommits }, signal }),
  branches: (repo) => request("GET", `${r(repo)}/branches`),
  tags: (repo) => request("GET", `${r(repo)}/tags`),
  events: (repo, since, limit) => request("GET", `${r(repo)}/events`, { query: { since, limit } }),
  streamEvents: (repo, since, onEvent, onStatus) => streamLive(repo, since, onEvent, onStatus),
  leases: (repo) => request("GET", `${r(repo)}/leases`),
  createLease: (repo, body) => request("POST", `${r(repo)}/leases`, { body }),
  renewLease: (repo, id, ttl_secs) => request("POST", `${r(repo)}/leases/${enc(id)}/renew`, { body: { ttl_secs } }),
  releaseLease: (repo, id) => request("DELETE", `${r(repo)}/leases/${enc(id)}`),
  checkLease: (repo, holder, paths) => request("POST", `${r(repo)}/leases/check`, { body: { holder, paths } }),
  pulls: (repo, state) => request("GET", `${r(repo)}/pulls`, { query: { state: state === "all" ? undefined : state } }),
  createPull: (repo, body) => request("POST", `${r(repo)}/pulls`, { body }),
  pull: (repo, n) => request("GET", `${r(repo)}/pulls/${n}`),
  pullDiff: (repo, n) => request("GET", `${r(repo)}/pulls/${n}/diff`),
  addPullComment: (repo, n, body) => request("POST", `${r(repo)}/pulls/${n}/comments`, { body }),
  resolvePullComment: (repo, n, id) => request("POST", `${r(repo)}/pulls/${n}/comments/${id}/resolve`),
  reviewPull: (repo, n, body) => request("POST", `${r(repo)}/pulls/${n}/reviews`, { body }),
  addPullCheck: (repo, n, body) => request("POST", `${r(repo)}/pulls/${n}/checks`, { body }),
  mergePull: (repo, n, body) => request("POST", `${r(repo)}/pulls/${n}/merge`, { body }),
  closePull: (repo, n) => request("POST", `${r(repo)}/pulls/${n}/close`),
  updatePull: (repo, n, body) => request("POST", `${r(repo)}/pulls/${n}/update`, { body }),
  notes: (repo, q = {}) => request("GET", `${r(repo)}/knowledge/notes`, { query: { tags: q.tags?.length ? q.tags.join(",") : undefined, path: q.path, q: q.q, commit: q.commit, pull: q.pull, drafts: q.drafts ? 1 : undefined, include_superseded: q.include_superseded ? 1 : undefined, limit: q.limit } }),
  note: (repo, id) => request("GET", `${r(repo)}/knowledge/notes/${enc(id)}`),
  createNote: (repo, body) => request("POST", `${r(repo)}/knowledge/notes`, { body }),
  patchNote: (repo, id, body) => request("PATCH", `${r(repo)}/knowledge/notes/${enc(id)}`, { body }),
  deleteNote: (repo, id) => request("DELETE", `${r(repo)}/knowledge/notes/${enc(id)}`),
  validateNote: (repo, id, rev) => request("POST", `${r(repo)}/knowledge/notes/${enc(id)}/validate`, { body: rev ? { rev } : {} }),
  acceptNote: (repo, id, patch) => request("POST", `${r(repo)}/knowledge/notes/${enc(id)}/accept`, { body: patch ?? {} }),
  staleNotes: (repo, q = {}) => request("GET", `${r(repo)}/knowledge/stale`, { query: { rev: q.rev, drafts: q.drafts ? 1 : undefined } }),
  tagCounts: (repo) => request("GET", `${r(repo)}/knowledge/tags`),
  rules: (repo) => request("GET", `${r(repo)}/knowledge/rules`),
  putRules: (repo, rules) => request("PUT", `${r(repo)}/knowledge/rules`, { body: rules }),
  context: (repo, path, q = {}) => request("GET", `${r(repo)}/context`, { query: { path, limit: q.limit, include: q.files ? "files" : undefined, budget: q.files ? q.budget : undefined } }),
  policy: (repo) => request("GET", `${r(repo)}/policy`),
  putPolicy: (repo, policy) => request("PUT", `${r(repo)}/policy`, { body: policy }),
  keys: (repo) => request("GET", `${r(repo)}/keys`),
  addKey: (repo, body) => request("POST", `${r(repo)}/keys`, { body }),
  deleteKey: (repo, keyId) => request("DELETE", `${r(repo)}/keys/${enc(keyId)}`),
  webhooks: (repo) => request("GET", `${r(repo)}/webhooks`),
  createWebhook: (repo, body) => request("POST", `${r(repo)}/webhooks`, { body }),
  deleteWebhook: (repo, id) => request("DELETE", `${r(repo)}/webhooks/${enc(id)}`),
  testWebhook: (repo, id) => request("POST", `${r(repo)}/webhooks/${enc(id)}/test`),
  agents: (repo) => request("GET", `${r(repo)}/agents`),
};

/** SSE via EventSource; falls back to polling `GET /events?since=` on failure. */
function streamLive(repo: string, since: number, onEvent: (e: T.Event) => void, onStatus?: (s: StreamStatus) => void): () => void {
  let last = since;
  let closed = false;
  let es: EventSource | null = null;
  let pollTimer: ReturnType<typeof setTimeout> | null = null;
  const s = loadSettings();

  const handle = (e: T.Event) => {
    if (e.seq > last) last = e.seq;
    onEvent(e);
  };

  const poll = async () => {
    if (closed) return;
    try {
      const evs = await liveApi.events(repo, last, 200);
      evs.forEach(handle);
      onStatus?.("polling");
    } catch (e) {
      onStatus?.("error");
      // 401/403/404 will not fix themselves: stop instead of polling a missing repo forever.
      if (e instanceof ApiError && [401, 403, 404].includes(e.status)) {
        closed = true;
        return;
      }
    }
    if (!closed) pollTimer = setTimeout(poll, 3000);
  };

  const startPolling = () => {
    if (es) {
      es.close();
      es = null;
    }
    if (pollTimer) return;
    void poll();
  };

  onStatus?.("connecting");
  // EventSource cannot set headers. When a token is configured we skip SSE and
  // poll with fetch, which carries Authorization / X-Agit-Agent.
  if (typeof EventSource === "undefined" || s.token) {
    startPolling();
  } else {
    try {
      es = new EventSource(buildUrl(s.baseUrl, `${r(repo)}/events/stream`, { since: last }));
      let opened = false;
      es.onopen = () => {
        opened = true;
        onStatus?.("sse");
      };
      es.onmessage = (m) => {
        try {
          handle(JSON.parse(m.data) as T.Event);
        } catch {
          /* ignore malformed frame */
        }
      };
      es.onerror = () => {
        // Browsers auto-reconnect; if we never opened, or it keeps failing, poll.
        if (!opened || es?.readyState === EventSource.CLOSED) startPolling();
      };
    } catch {
      startPolling();
    }
  }

  return () => {
    closed = true;
    es?.close();
    if (pollTimer) clearTimeout(pollTimer);
  };
}

// ---- Facade with mode switching --------------------------------------------

export type ApiMode = "live" | "mock" | "unknown";
let current: Api = liveApi;
let mode: ApiMode = "unknown";
const modeListeners = new Set<(m: ApiMode) => void>();

export function getApiMode() {
  return mode;
}
export function onApiMode(l: (m: ApiMode) => void) {
  modeListeners.add(l);
  return () => {
    modeListeners.delete(l);
  };
}
function setMode(m: ApiMode, impl: Api) {
  mode = m;
  current = impl;
  modeListeners.forEach((l) => l(m));
}

let initPromise: Promise<ApiMode> | null = null;

/** Decide between live and mock once. Safe to call repeatedly. */
export function initApi(force = false): Promise<ApiMode> {
  if (initPromise && !force) return initPromise;
  initPromise = (async () => {
    const forceMock = import.meta.env.VITE_AGIT_MOCK === "1" || import.meta.env.VITE_AGIT_MOCK === "true";
    if (!forceMock) {
      try {
        const ctl = new AbortController();
        const t = setTimeout(() => ctl.abort(), 2500);
        await request<T.Health>("GET", "/health", { signal: ctl.signal });
        clearTimeout(t);
        setMode("live", liveApi);
        return "live" as const;
      } catch {
        /* fall through to mock */
      }
    }
    const { mockApi } = await import("./mock");
    setMode("mock", mockApi);
    return "mock" as const;
  })();
  return initPromise;
}

/** Proxy that forwards to whichever implementation is active. */
export const api: Api = new Proxy({} as Api, {
  get(_t, prop: keyof Api) {
    return (...args: unknown[]) => (current[prop] as (...a: unknown[]) => unknown)(...args);
  },
});
