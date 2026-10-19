// TypeScript types for every object in docs/API.md (agit server HTTP API v1).
// Keep this file in sync with the spec; the web UI consumes nothing else.

export type ObjectId = string; // 64-char hex BLAKE3

// ---- Error envelope ---------------------------------------------------------

export interface ApiErrorBody {
  ok: false;
  error: {
    code: string;
    message: string;
    retryable: boolean;
    details?: unknown;
  };
}

// ---- Health -----------------------------------------------------------------

export interface Health {
  status: string;
  version: string;
}

// ---- Repositories -----------------------------------------------------------

export interface RepoSummary {
  name: string;
  default_branch: string;
  head: ObjectId | null;
  created_ms: number;
}

export interface RepoDetail extends RepoSummary {
  branches: number;
  tags: number;
  objects: number;
}

// ---- Refs -------------------------------------------------------------------

export interface Ref {
  name: string;
  id: ObjectId;
}

export type RefExpect = { any: true } | { absent: true } | { is: ObjectId };

export interface RefUpdateRequest {
  updates: { name: string; expect: RefExpect; new: ObjectId | null }[];
  message?: string;
}

// ---- Objects ----------------------------------------------------------------

export type EntryMode = "file" | "executable" | "symlink" | "dir" | "submodule";

export interface TreeEntry {
  name: string;
  mode: EntryMode;
  id: ObjectId;
  size: number;
  /** With `last_commits=1`: the most recent commit that changed this entry. */
  last_commit?: BlameCommit;
}

export interface Signature {
  name: string;
  email: string;
}

export interface AgentInfo {
  id: string;
  model?: string | null;
  run_id?: string | null;
  on_behalf_of?: string | null;
}

export interface Commit {
  tree: ObjectId;
  parents: ObjectId[];
  author: Signature;
  time_ms: number;
  message: string;
  intent?: string | null;
  task?: string | null;
  agent?: AgentInfo | null;
  checkpoint?: boolean;
  meta?: Record<string, unknown>;
}

export interface ObjectJson {
  id: ObjectId;
  kind: "blob" | "tree" | "commit";
  size: number;
  tree?: TreeEntry[];
  commit?: Commit;
  text?: string;
}

// ---- Browsing ---------------------------------------------------------------

/** Item of `GET /repos/{repo}/commits`: `{id, ...commit}`. */
export interface CommitListItem extends Commit {
  id: ObjectId;
}

export type ChangeKind = "added" | "modified" | "deleted" | "type_changed" | "renamed";

export interface ChangeSide {
  id: ObjectId;
  mode: EntryMode;
  size: number;
}

export interface Change {
  path: string;
  kind: ChangeKind;
  old?: ChangeSide | null;
  new?: ChangeSide | null;
  /** Previous path; only with `kind: "renamed"` (rename detection). */
  from?: string | null;
  /** Similarity in percent for renames (100 = identical content). */
  similarity?: number | null;
}

/**
 * `CommitDetail.signature`: `agit_core::sign::SignatureStatus` flattened with
 * `trusted` (key registered for the signer under `/keys`).
 */
export type SignatureInfo =
  | { status: "unsigned"; trusted: boolean }
  | { status: "valid"; key_id: string; signer: string; public_key: string; trusted: boolean }
  | { status: "invalid"; reason: string; trusted: boolean };

export interface CommitDetail {
  id: ObjectId;
  commit: Commit;
  changes: Change[];
  patch: string;
  signature?: SignatureInfo | null;
  /** Up to 10 notes: those linking the commit (drafts included) first, then accepted notes covering a changed path. */
  notes: NoteSummary[];
}

/** `GET /repos/{repo}/blame/{rev}?path=` (`agit_core::history::Blame`). */
export interface BlameLine {
  /** 1-based line number in the blamed revision. */
  line: number;
  commit: ObjectId;
  text: string;
}

export interface BlameCommit {
  id: ObjectId;
  author: string;
  agent?: string | null;
  time_ms: number;
  summary: string;
  intent?: string | null;
}

export interface Blame {
  path: string;
  rev: ObjectId;
  lines: BlameLine[];
  commits: Record<ObjectId, BlameCommit>;
  /** The history walk hit `max_commits`; older lines are attributed to the oldest commit examined. */
  truncated: boolean;
}

/** `GET/POST /repos/{repo}/keys`. */
export interface KeyEntry {
  agent: string;
  key_id: string;
  public_key: string;
  added_ms: number;
  added_by: string;
}

export interface KeyCreateRequest {
  agent: string;
  public_key: string;
}

export interface TreeDir {
  kind: "tree";
  path: string;
  id: ObjectId;
  entries: TreeEntry[];
  /** With `last_commits=1`: the history walk stopped before every entry was resolved. */
  last_commits_truncated?: boolean;
}

export interface TreeBlob {
  kind: "blob";
  path: string;
  id: ObjectId;
  size: number;
  binary: boolean;
  text?: string | null;
}

export type TreeResponse = TreeDir | TreeBlob;

/** Per-file line statistics, as returned by `/diff` and `/pulls/{n}/diff`. */
export interface FileStat {
  path: string;
  added: number;
  removed: number;
}
export type DiffStats = FileStat[];

export interface Diff {
  changes: Change[];
  patch: string;
  stats: DiffStats;
}

export interface Branch {
  name: string;
  id: ObjectId;
  ahead?: number | null;
  behind?: number | null;
}

export interface Tag {
  name: string;
  id: ObjectId;
}

// ---- Events -----------------------------------------------------------------

export interface Event {
  seq: number;
  time_ms: number;
  kind: string;
  workspace?: string | null;
  actor?: string | null;
  data: Record<string, unknown>;
}

// ---- Leases -----------------------------------------------------------------

export type LeaseMode = "exclusive" | "shared";

export interface Lease {
  id: string;
  holder: string;
  paths: string[];
  mode: LeaseMode;
  created_ms: number;
  expires_ms?: number | null;
  note?: string | null;
  workspace?: string | null;
}

export interface LeaseCreateRequest {
  holder: string;
  paths: string[];
  mode: LeaseMode;
  ttl_secs: number;
  note?: string;
}

// ---- Pull requests ----------------------------------------------------------

export type PullState = "open" | "merged" | "closed";
export type ReviewDecision = "approve" | "request_changes" | "comment";
export type CommentKind = "question" | "suggestion" | "blocker" | "note";
export type CheckStatus = "pending" | "success" | "failure";

export interface PullAuthor {
  id: string;
  model: string | null;
}

export interface Review {
  reviewer: string;
  decision: ReviewDecision;
  body: string;
  time_ms: number;
}

export interface PullComment {
  id: number;
  author: string;
  path: string | null;
  line: number | null;
  kind: CommentKind;
  body: string;
  resolved: boolean;
  time_ms: number;
}

export interface Check {
  name: string;
  status: CheckStatus;
  url: string | null;
  time_ms: number;
}

export interface Pull {
  number: number;
  title: string;
  body: string;
  intent: string | null;
  task: string | null;
  source: string;
  target: string;
  source_id: ObjectId;
  target_id: ObjectId;
  base_id: ObjectId;
  author: PullAuthor;
  state: PullState;
  created_ms: number;
  updated_ms: number;
  merged_commit: ObjectId | null;
  reviews: Review[];
  comments: PullComment[];
  checks: Check[];
}

export interface PullDetail {
  pr: Pull;
  commits: CommitListItem[];
  changes: Change[];
  mergeable: boolean;
  conflicts: string[];
}

export interface PullCreateRequest {
  title: string;
  body?: string;
  intent?: string;
  task?: string;
  source: string;
  target?: string;
}

export interface PullCommentRequest {
  path?: string;
  line?: number;
  kind: CommentKind;
  body: string;
}

export interface PullReviewRequest {
  decision: ReviewDecision;
  body: string;
}

export interface PullCheckRequest {
  name: string;
  status: CheckStatus;
  url?: string;
}

export type MergeStrategy = "merge" | "ff";

export interface PullMergeRequest {
  strategy: MergeStrategy;
  message?: string;
}

export interface PullMergeResponse {
  pr: Pull;
  merged_commit: ObjectId;
}

export interface PullUpdateRequest {
  title?: string;
  body?: string;
  intent?: string;
  task?: string;
}

// ---- Knowledge notes --------------------------------------------------------

export interface Note {
  id: string;
  title: string;
  body: string;
  tags: string[];
  paths: string[];
  commits: ObjectId[];
  pulls: number[];
  author: string;
  confidence: number;
  created_ms: number;
  updated_ms: number;
  /** Absent or null when the note is current. */
  superseded_by?: string | null;
  /** Commit the note was written or last validated against. */
  valid_at?: ObjectId | null;
  /** Machine-generated (e.g. from a merged PR); hidden until accepted. Omitted when false. */
  draft?: boolean;
  /** Computed on read: a covered path changed between `valid_at` and HEAD. */
  stale?: boolean;
}

/** Item of `GET /knowledge/notes`: a subset of `Note` plus `excerpt` (no body, commits, pulls or created_ms). */
export interface NoteSummary {
  id: string;
  title: string;
  tags: string[];
  paths: string[];
  author: string;
  confidence: number;
  updated_ms: number;
  excerpt: string;
  superseded_by?: string | null;
  valid_at?: ObjectId | null;
  draft: boolean;
  stale: boolean;
  /** BM25 relevance; only present for `q=` queries. */
  score?: number | null;
}

/** Item of `GET /knowledge/stale`. */
export interface StaleNote extends NoteSummary {
  changed_paths: string[];
}

export interface NoteCreateRequest {
  title: string;
  body: string;
  tags: string[];
  paths?: string[];
  commits?: ObjectId[];
  pulls?: number[];
  confidence?: number;
  /** Defaults to HEAD of the default branch. */
  valid_at?: ObjectId;
  draft?: boolean;
}

export interface NoteListQuery {
  tags?: string[];
  path?: string;
  q?: string;
  commit?: string;
  pull?: number;
  drafts?: boolean;
  include_superseded?: boolean;
  limit?: number;
}

export type NotePatchRequest = Partial<NoteCreateRequest> & { superseded_by?: string | null };

export interface TagCount {
  tag: string;
  count: number;
}

export interface TagRule {
  pattern: string;
  tags: string[];
}

export interface ContextFile {
  path: string;
  text: string;
  truncated: boolean;
}

export interface Context {
  path: string;
  /** Fresh notes first, then stale ones; drafts excluded. */
  notes: NoteSummary[];
  suggested_tags: string[];
  recent_commits: CommitListItem[];
  leases: Lease[];
  open_pulls: Pull[];
  /** Agents holding a lease on the path or who committed to it in the last 24h. */
  agents_active: string[];
  /** Only with `include=files`; trimmed to `budget * 4` characters. */
  files?: ContextFile[];
}

export interface ContextQuery {
  limit?: number;
  /** `include=files`. */
  files?: boolean;
  /** Approximate token budget for `files` (default 6000). */
  budget?: number;
}

// ---- Policy -----------------------------------------------------------------

/** `GET/PUT /repos/{repo}/policy`. Field names follow `agit_protocol::Policy`. */
export interface Policy {
  /** Full ref names (`refs/heads/main`), branch names or globs (`release/*`). */
  protected_branches: string[];
  require_pull_request: boolean;
  require_approvals: number;
  require_intent: boolean;
  require_agent: boolean;
  require_checks: boolean;
  block_on_blockers: boolean;
  allowed_merge_agents: string[];
  allowed_push_agents: string[];
  /** 0 = unlimited. */
  max_files_per_commit: number;
  /** Commits reaching a protected branch must carry a valid Ed25519 signature with a key registered under `/keys`. */
  require_signed: boolean;
}

export const DEFAULT_POLICY: Policy = {
  protected_branches: [],
  require_pull_request: false,
  require_approvals: 0,
  require_intent: false,
  require_agent: false,
  require_checks: false,
  block_on_blockers: true,
  allowed_merge_agents: [],
  allowed_push_agents: [],
  max_files_per_commit: 0,
  require_signed: false,
};

// ---- Webhooks ---------------------------------------------------------------

export interface Webhook {
  id: string;
  url: string;
  /** Exact kinds (`commit`), prefixes (`pull.*`) or empty for all. */
  kinds: string[];
  active: boolean;
  /** The secret itself is never returned. */
  has_secret: boolean;
  created_ms: number;
}

export interface WebhookCreateRequest {
  url: string;
  kinds?: string[];
  secret?: string;
}

// ---- Activity ---------------------------------------------------------------

export interface AgentActivity {
  id: string;
  model?: string | null;
  commits: number;
  last_seen_ms: number;
  open_pulls: number;
  leases: number;
}

// ---- Auth -------------------------------------------------------------------

export type Access = "read" | "write" | "admin";

export interface Scope {
  /** Repository name, or `*` for every repository. */
  repo: string;
  access: Access;
}

export interface TokenCreateRequest {
  name: string;
  /** Bind the token to an agent identity (overrides `X-Agit-Agent`). */
  agent?: string;
  scopes: Scope[];
  ttl_secs?: number;
}

export interface TokenInfo {
  id: string;
  name: string;
  agent?: string | null;
  scopes: Scope[];
  created_ms: number;
  expires_ms?: number | null;
  created_by: string;
  last_used_ms?: number | null;
  /** Session tokens: the authorized key (`/auth/keys` id, or `admin` for a startup admin key) that signed in. */
  key?: string | null;
}

/** An SSH public key allowed to sign in (`GET /auth/keys`). */
export interface AuthorizedKey {
  id: string;
  name: string;
  /** `SHA256:…`, as `ssh-keygen -lf` prints it. */
  fingerprint: string;
  key_id: string;
  /** `ssh-ed25519 <base64> [comment]`. */
  public_key: string;
  /** Sessions opened with this key always act as this agent. */
  agent?: string | null;
  scopes: Scope[];
  created_ms: number;
  created_by: string;
  last_used_ms?: number | null;
}

export interface AuthorizedKeyCreateRequest {
  /** `ssh-ed25519 …` line or a 64-hex Ed25519 key. */
  public_key: string;
  name?: string;
  agent?: string;
  scopes: Scope[];
}

/** `POST /auth/challenge`: a single-use nonce to sign with an SSH key. */
export interface Challenge {
  nonce: string;
  expires_ms: number;
}

/** `POST /auth/session`: hex Ed25519 signature over `agit-auth-v1\n<nonce>`. */
export interface SessionCreateRequest {
  public_key: string;
  nonce: string;
  signature: string;
  ttl_secs?: number;
}

/** `POST /auth/tokens`: the plaintext `token` is returned exactly once. */
export interface TokenCreated extends TokenInfo {
  token: string;
}

export interface WhoAmI {
  /** `root`, `anonymous`, or the token id. */
  principal: string;
  agent?: string | null;
  root: boolean;
  scopes: Scope[];
  /** The authorized key behind this session, when signed in with one. */
  key?: string | null;
}
