// `AgentSession`: the docs/AGENT-GUIDE.md loop (read context, claim, work, open a PR,
// record what you learned) bound to one client, repository and agent id.

import type { AgitClient, StreamEventsOptions } from "./client.js";
import { NotFoundError } from "./errors.js";
import type * as T from "./types.js";

export interface AgentSessionOptions {
  /** Lease TTL used when `claim` is called without `ttlSecs`. Default 1800 (30 min). */
  defaultTtlSecs?: number;
  /** Model name for the `X-Agit-Model` header; defaults to the client's. */
  model?: string;
}

export interface ClaimOptions {
  ttlSecs?: number;
  note?: string;
  /** Default `exclusive`. */
  mode?: T.LeaseMode;
}

export interface OpenPullRequestOptions {
  title: string;
  /** Source ref or branch name (`ws/agent1` is expanded to `refs/heads/ws/agent1`). */
  source: string;
  /** Target ref or branch name; defaults to the repository's default branch. */
  target?: string;
  body?: string;
  intent?: string;
  task?: string;
}

export interface RememberOptions {
  title: string;
  body: string;
  tags?: string[];
  paths?: string[];
  commits?: T.ObjectId[];
  pulls?: number[];
  confidence?: number;
}

function fullRef(name: string): string {
  return name.startsWith("refs/") ? name : `refs/heads/${name}`;
}

export class AgentSession {
  readonly client: AgitClient;
  readonly repo: string;
  readonly agentId: string;
  private readonly defaultTtlSecs: number;
  private readonly held = new Map<string, T.Lease>();

  constructor(client: AgitClient, repo: string, agentId: string, opts: AgentSessionOptions = {}) {
    // Make sure the server attributes leases, PRs, reviews and notes to `agentId`.
    this.client = client.agent === agentId && (opts.model === undefined || client.model === opts.model) ? client : client.withAgent(agentId, opts.model);
    this.repo = repo;
    this.agentId = agentId;
    this.defaultTtlSecs = opts.defaultTtlSecs ?? 1800;
  }

  /** Leases claimed through this session and not yet released. */
  get leases(): T.Lease[] {
    return Array.from(this.held.values());
  }

  /** Notes, suggested tags, recent commits, leases and open PRs touching `path`. */
  context(path: string, limit?: number): Promise<T.ContextResponse> {
    return this.client.getContext(this.repo, path, limit);
  }

  /** Claim `paths` for this agent. Throws `LeaseConflictError` when someone else holds them. */
  async claim(paths: string | string[], opts: ClaimOptions = {}): Promise<T.Lease> {
    const lease = await this.client.claimLease(this.repo, {
      holder: this.agentId,
      paths: Array.isArray(paths) ? paths : [paths],
      mode: opts.mode ?? "exclusive",
      ttl_secs: opts.ttlSecs ?? this.defaultTtlSecs,
      ...(opts.note !== undefined ? { note: opts.note } : {}),
    });
    this.held.set(lease.id, lease);
    return lease;
  }

  /** Resolves when this agent may write `paths` (no foreign exclusive lease); throws `LeaseConflictError` otherwise. */
  check(paths: string | string[]): Promise<T.LeaseCheckResponse> {
    return this.client.checkLease(this.repo, this.agentId, Array.isArray(paths) ? paths : [paths]);
  }

  /** Extend one lease (default: every lease held by this session). */
  async renew(ttlSecs?: number, leaseId?: string): Promise<T.Lease[]> {
    const ids = leaseId ? [leaseId] : Array.from(this.held.keys());
    const out: T.Lease[] = [];
    for (const id of ids) {
      const l = await this.client.renewLease(this.repo, id, ttlSecs ?? this.defaultTtlSecs);
      this.held.set(l.id, l);
      out.push(l);
    }
    return out;
  }

  /** Release one lease. Already-expired/released leases are ignored. */
  async release(leaseId: string): Promise<void> {
    try {
      await this.client.releaseLease(this.repo, leaseId);
    } catch (e) {
      if (!(e instanceof NotFoundError)) throw e;
    } finally {
      this.held.delete(leaseId);
    }
  }

  /** Release every lease held by this session. Continues past individual failures and rethrows the first one. */
  async releaseAll(): Promise<void> {
    let firstError: unknown;
    for (const id of Array.from(this.held.keys())) {
      try {
        await this.release(id);
      } catch (e) {
        firstError ??= e;
      }
    }
    if (firstError !== undefined) throw firstError;
  }

  /** Claim `paths`, run `fn`, and release in `finally` even when `fn` throws. */
  async withLeases<R>(paths: string | string[], fn: (lease: T.Lease) => Promise<R> | R, opts: ClaimOptions = {}): Promise<R> {
    const lease = await this.claim(paths, opts);
    try {
      return await fn(lease);
    } finally {
      await this.release(lease.id);
    }
  }

  /** Open a pull request from this agent's branch. */
  openPullRequest(opts: OpenPullRequestOptions): Promise<T.PullRequest> {
    const body: T.PullCreateRequest = { title: opts.title, source: fullRef(opts.source) };
    if (opts.target !== undefined) body.target = fullRef(opts.target);
    if (opts.body !== undefined) body.body = opts.body;
    if (opts.intent !== undefined) body.intent = opts.intent;
    if (opts.task !== undefined) body.task = opts.task;
    return this.client.createPull(this.repo, body);
  }

  /** Record a knowledge note so the next agent touching these paths sees it. */
  remember(opts: RememberOptions): Promise<T.Note> {
    const body: T.NoteCreateRequest = { title: opts.title, body: opts.body };
    if (opts.tags) body.tags = opts.tags;
    if (opts.paths) body.paths = opts.paths;
    if (opts.commits) body.commits = opts.commits;
    if (opts.pulls) body.pulls = opts.pulls;
    if (opts.confidence !== undefined) body.confidence = opts.confidence;
    return this.client.createNote(this.repo, body);
  }

  /** Live events of this repository (see `AgitClient.streamEvents`). */
  events(opts?: StreamEventsOptions): AsyncGenerator<T.Event, void, undefined> {
    return this.client.streamEvents(this.repo, opts);
  }
}
