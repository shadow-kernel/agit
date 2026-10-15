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

export type ChangeKind = "added" | "modified" | "deleted" | "type_changed";

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
}

export interface CommitDetail {
  id: ObjectId;
  commit: Commit;
  changes: Change[];
  patch: string;
}

export interface TreeDir {
  kind: "tree";
  path: string;
  entries: TreeEntry[];
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

export interface DiffStats {
  files?: number;
  insertions?: number;
  deletions?: number;
  [key: string]: unknown;
}

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
  superseded_by: string | null;
}

/** Note without `body`, plus `excerpt`. */
export interface NoteSummary extends Omit<Note, "body"> {
  excerpt: string;
}

export interface NoteCreateRequest {
  title: string;
  body: string;
  tags: string[];
  paths?: string[];
  commits?: ObjectId[];
  pulls?: number[];
  confidence?: number;
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

export interface Context {
  notes: NoteSummary[];
  recent_commits: CommitListItem[];
  leases: Lease[];
  open_pulls: Pull[];
}

// ---- Activity ---------------------------------------------------------------

export interface AgentActivity {
  id: string;
  commits: number;
  last_seen_ms: number;
  open_pulls: number;
  leases: number;
}
