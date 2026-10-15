// Types for every object of the agit server HTTP API v1 (docs/API.md).
// Field names and enum values mirror crates/agit-protocol/src/types.rs exactly.

/** 64-char lowercase hex BLAKE3 digest. */
export type ObjectId = string;

// ---- Error envelope ---------------------------------------------------------

export interface ApiErrorPayload {
  /** Stable machine-readable code, e.g. `ref_conflict`, `lease_conflict`, `not_found`. */
  code: string;
  message: string;
  /** Whether the same operation may sensibly be retried. */
  retryable: boolean;
  /** Code-specific payload, e.g. `{name, expected, actual}` for `ref_conflict`. */
  details?: unknown;
}

export interface ApiErrorBody {
  ok: false;
  error: ApiErrorPayload;
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
  /** Absent/null while the repository has no commits. */
  head?: ObjectId | null;
  created_ms: number;
  branches?: number;
  tags?: number;
  objects?: number;
}

/** `GET /repos/{repo}`: summary plus counters. */
export interface RepoDetail extends RepoSummary {
  branches: number;
  tags: number;
  objects: number;
}

export interface CreateRepoRequest {
  name: string;
  default_branch?: string;
}

export interface DeleteRepoResponse {
  deleted: true;
}

// ---- Refs -------------------------------------------------------------------

export interface RefEntry {
  name: string;
  id: ObjectId;
}

export interface BranchEntry {
  name: string;
  id: ObjectId;
  ahead?: number | null;
  behind?: number | null;
}

/** Compare-and-swap precondition: `{any:true}` | `{absent:true}` | `{is:"<id>"}`. */
export type ExpectSpec = { any: true } | { absent: true } | { is: ObjectId };

export interface RefUpdate {
  name: string;
  expect: ExpectSpec;
  /** New value; `null` deletes the ref. */
  new: ObjectId | null;
}

export interface RefUpdateRequest {
  updates: RefUpdate[];
  message?: string;
}

export interface RefUpdateResponse {
  updated: RefEntry[];
  deleted: string[];
}

/** `details` of a `ref_conflict` error. */
export interface RefConflictDetails {
  name: string;
  expected: string;
  actual: string | null;
}

// ---- Objects ----------------------------------------------------------------

export type ObjectKind = "blob" | "tree" | "commit";

export type EntryMode = "file" | "executable" | "symlink" | "dir" | "submodule";

export interface TreeEntry {
  name: string;
  mode: EntryMode;
  id: ObjectId;
  /** Blob size in bytes, or number of entries for a directory. */
  size: number;
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

export interface MissingObjectsResponse {
  missing: ObjectId[];
}

export interface StoreObjectsResponse {
  stored: number;
  skipped: number;
}

export interface FetchObjectsRequest {
  wants: ObjectId[];
  haves?: ObjectId[];
}

/** `GET /repos/{repo}/objects/{id}/json`. */
export type ObjectJson =
  | { id: ObjectId; kind: "blob"; size: number; binary: boolean; text: string | null }
  | { id: ObjectId; kind: "tree"; size: number; tree: TreeEntry[] }
  | { id: ObjectId; kind: "commit"; commit: Commit };

/** Raw object payload with its kind (from the `X-Agit-Kind` header). */
export interface RawObject {
  kind: ObjectKind;
  payload: Uint8Array;
}

// ---- Browsing ---------------------------------------------------------------

/** Item of `GET /repos/{repo}/commits`: `{id, ...commit}` (commit fields flattened). */
export interface CommitEntry extends Commit {
  id: ObjectId;
}

/** `renamed` only appears when rename detection is applied (CLI `diff`/`show`); the server's own diffs use the first four. */
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
  /** Previous path, for `kind: "renamed"`. */
  from?: string;
  /** Similarity in percent, for `kind: "renamed"`. */
  similarity?: number;
}

export interface CommitDetail {
  id: ObjectId;
  commit: Commit;
  changes: Change[];
  patch: string;
  /** Signature verdict plus whether the key is registered for the signer; absent on older servers. */
  signature?: SignatureInfo;
  /** Up to 10 notes: those linking the commit (drafts included) first, then accepted notes covering a changed path. */
  notes: NoteSummary[];
}

// ---- Signing keys -----------------------------------------------------------

/** Outcome of verifying a commit's Ed25519 signature (`agit_core::sign::SignatureStatus`, tagged by `status`). */
export type SignatureStatus =
  | { status: "unsigned" }
  | { status: "valid"; key_id: string; signer: string; public_key: string }
  | { status: "invalid"; reason: string };

/** `CommitDetail.signature`: the status flattened with `trusted` (key registered for the signer in `/keys`). */
export type SignatureInfo = SignatureStatus & { trusted: boolean };

/** A public signing key registered for an agent (`GET /repos/{repo}/keys`). */
export interface KeyEntry {
  agent: string;
  key_id: string;
  /** Hex-encoded Ed25519 public key (32 bytes). */
  public_key: string;
  added_ms: number;
  added_by: string;
}

export interface KeyCreate {
  agent: string;
  /** Hex-encoded Ed25519 public key, as printed by `agit key generate`. */
  public_key: string;
}

export interface DeleteKeyResponse {
  deleted: string;
}

export interface FileStat {
  path: string;
  added: number;
  removed: number;
}

export interface DiffResponse {
  changes: Change[];
  patch: string;
  stats: FileStat[];
}

export interface TreeDirResponse {
  kind: "tree";
  path: string;
  id: ObjectId;
  entries: TreeEntry[];
}

export interface TreeBlobResponse {
  kind: "blob";
  path: string;
  id: ObjectId;
  size: number;
  binary: boolean;
  text?: string | null;
}

export type TreeResponse = TreeDirResponse | TreeBlobResponse;

export interface ListCommitsQuery {
  /** Revision to start from (default `HEAD`). */
  rev?: string;
  limit?: number;
  agent?: string;
  task?: string;
  path?: string;
  first_parent?: boolean;
}

// ---- Events -----------------------------------------------------------------

export interface Event {
  /** Monotonic sequence number within the repository. */
  seq: number;
  time_ms: number;
  /** e.g. `commit`, `ref.update`, `lease.claim`, `pull.open`, `pull.merge`, `note.create`. */
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
  /** Path patterns; a plain path covers itself and everything beneath it. */
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
  /** Defaults to `exclusive` on the server. */
  mode?: LeaseMode;
  ttl_secs?: number;
  note?: string;
}

export interface LeaseCheckResponse {
  allowed: true;
  holder: string;
  paths: string[];
}

export interface LeaseReleaseResponse {
  released: string;
}

/** `details` of a `lease_conflict` error. */
export interface LeaseConflictDetails {
  path: string;
  holder: string;
  lease_id: string;
}

// ---- Pull requests ----------------------------------------------------------

export type PullState = "open" | "merged" | "closed";
export type ReviewDecision = "approve" | "request_changes" | "comment";
export type CommentKind = "question" | "suggestion" | "blocker" | "note";
export type CheckStatus = "pending" | "success" | "failure";
export type MergeStrategy = "merge" | "ff";

export interface Actor {
  id: string;
  model?: string | null;
}

export interface Review {
  reviewer: string;
  decision: ReviewDecision;
  body: string;
  time_ms: number;
}

export interface Comment {
  id: number;
  author: string;
  path?: string | null;
  line?: number | null;
  kind: CommentKind;
  body: string;
  resolved: boolean;
  time_ms: number;
}

export interface Check {
  name: string;
  status: CheckStatus;
  url?: string | null;
  time_ms: number;
}

export interface PullRequest {
  number: number;
  title: string;
  body: string;
  intent?: string | null;
  task?: string | null;
  source: string;
  target: string;
  source_id: ObjectId;
  target_id: ObjectId;
  base_id?: ObjectId | null;
  author: Actor;
  state: PullState;
  created_ms: number;
  updated_ms: number;
  merged_commit?: ObjectId | null;
  reviews: Review[];
  comments: Comment[];
  checks: Check[];
}

export type ConflictKind = "content" | "delete_modify" | "add_add" | "mode" | "binary";

export interface Conflict {
  path: string;
  kind: ConflictKind;
  base?: ObjectId | null;
  ours?: ObjectId | null;
  theirs?: ObjectId | null;
}

export interface PullDetail {
  pr: PullRequest;
  commits: CommitEntry[];
  changes: Change[];
  mergeable: boolean;
  conflicts: Conflict[];
}

export interface PullCreateRequest {
  title: string;
  body?: string;
  intent?: string;
  task?: string;
  /** Source ref, e.g. `refs/heads/ws/agent1`. */
  source: string;
  /** Target ref; defaults to the repository's default branch. */
  target?: string;
}

export interface PullUpdateRequest {
  title?: string;
  body?: string;
  intent?: string;
  task?: string;
}

export interface CommentCreateRequest {
  path?: string;
  line?: number;
  /** Defaults to `note` on the server. */
  kind?: CommentKind;
  body: string;
}

export interface ReviewCreateRequest {
  decision: ReviewDecision;
  body?: string;
}

export interface CheckCreateRequest {
  name: string;
  status: CheckStatus;
  url?: string;
}

export interface MergeRequest {
  /** Defaults to `merge` on the server. */
  strategy?: MergeStrategy;
  message?: string;
}

export interface MergeResponse {
  pr: PullRequest;
  merged_commit: ObjectId;
}

/** `details` of a `merge_conflicts` error. */
export interface MergeConflictsDetails {
  count: number;
}

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

export interface WebhookCreate {
  url: string;
  kinds?: string[];
  secret?: string;
}

// ---- Policy -----------------------------------------------------------------

export interface Policy {
  /** Full ref names (`refs/heads/main`) or branch globs (`release/*`). */
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
  /** Every commit reaching a protected branch needs a valid signature with a key registered for its signer. */
  require_signed: boolean;
}

// ---- Knowledge notes --------------------------------------------------------

export interface Note {
  id: string;
  title: string;
  /** Markdown. */
  body: string;
  /** `namespace:value` tags, e.g. `area:auth`, `kind:gotcha`. */
  tags: string[];
  /** Path globs this note is about, e.g. `src/auth/**`. */
  paths: string[];
  commits: ObjectId[];
  pulls: number[];
  author: string;
  confidence: number;
  created_ms: number;
  updated_ms: number;
  superseded_by?: string | null;
  /** Commit the note was written or last validated against (HEAD of the default branch at creation by default). */
  valid_at?: ObjectId | null;
  /** Machine-generated (e.g. from a merged PR) and hidden until accepted. Omitted when false. */
  draft?: boolean;
  /** Computed on every read: a path covered by `paths` changed between `valid_at` and HEAD. */
  stale: boolean;
}

/** Note without `body`, plus `excerpt`, `valid_at`, `draft`, `stale` and (for `q=` searches) `score`. */
export interface NoteSummary {
  id: string;
  title: string;
  tags: string[];
  paths: string[];
  author: string;
  confidence: number;
  updated_ms: number;
  /** First 200 characters of the body. */
  excerpt: string;
  superseded_by?: string | null;
  valid_at?: ObjectId | null;
  draft: boolean;
  stale: boolean;
  /** BM25 relevance; only present for full-text (`q=`) queries. */
  score?: number;
}

/** `GET /repos/{repo}/knowledge/stale`: a summary (`stale: true`) plus the covered paths that changed since `valid_at`. */
export interface StaleNote extends NoteSummary {
  stale: true;
  changed_paths: string[];
}

export interface NoteCreateRequest {
  title: string;
  body?: string;
  tags?: string[];
  paths?: string[];
  commits?: ObjectId[];
  pulls?: number[];
  confidence?: number;
  /** Must be an object the server has (412 `object_not_found`); defaults to HEAD of the default branch. */
  valid_at?: ObjectId;
  /** Create a hidden draft to be accepted later. */
  draft?: boolean;
}

/** Body of `POST /knowledge/notes/{id}/validate`. */
export interface ValidateNoteRequest {
  /** Revision the note was checked against; default HEAD. */
  rev?: string;
}

export interface NotePatchRequest {
  title?: string;
  body?: string;
  tags?: string[];
  paths?: string[];
  commits?: ObjectId[];
  pulls?: number[];
  confidence?: number;
  /** `null` clears the supersession. */
  superseded_by?: string | null;
}

export interface ListNotesQuery {
  /** All given tags must match. */
  tags?: string[];
  /** Notes whose `paths` cover this path. */
  path?: string;
  /** Full-text query (BM25 over title, tags, body and paths); results come best first with `score` set. */
  q?: string;
  /** Notes listing this commit (any revision syntax). */
  commit?: string;
  /** Notes listing this pull request number. */
  pull?: number;
  /** Default 100. */
  limit?: number;
  /** Include unaccepted drafts. */
  drafts?: boolean;
  include_superseded?: boolean;
}

export interface ListStaleNotesQuery {
  /** Revision to compare against; default HEAD. */
  rev?: string;
  /** Include drafts. */
  drafts?: boolean;
}

export interface DeleteNoteResponse {
  deleted: string;
}

export interface TagCount {
  tag: string;
  count: number;
}

export interface TagRule {
  pattern: string;
  tags: string[];
}

export interface ContextQuery {
  /** Caps `notes` and `recent_commits`; default 10. */
  limit?: number;
  /** Add `files` (the file at `path` plus up to 5 siblings) to the response. */
  include_files?: boolean;
  /** Approximate token budget for the whole response (4 chars per token); default 6000. Only with `include_files`. */
  budget?: number;
}

/** One file of a context pack (`include=files`). */
export interface ContextFile {
  path: string;
  text: string;
  /** The text was cut to fit the budget. */
  truncated: boolean;
}

/** `GET /repos/{repo}/context?path=`: everything an agent should read before touching a path. */
export interface ContextResponse {
  path: string;
  /** Fresh notes first, then stale ones (`stale: true`); drafts excluded. */
  notes: NoteSummary[];
  suggested_tags: string[];
  recent_commits: CommitEntry[];
  leases: Lease[];
  open_pulls: PullRequest[];
  /** Sorted agent ids holding a lease overlapping the path or who committed to it in the last 24 hours. */
  agents_active: string[];
  /** Only present with `include=files`; most recently touched first, binary files skipped. */
  files?: ContextFile[];
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
