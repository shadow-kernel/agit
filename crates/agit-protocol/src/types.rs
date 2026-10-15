//! JSON types of the HTTP API (docs/API.md).

use agit_core::diff::Change;
use agit_core::lease::{Lease, LeaseMode};
use agit_core::merge::Conflict;
use agit_core::{Commit, ObjectId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiErrorBody {
    pub ok: bool,
    pub error: ApiError,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Health {
    pub status: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoSummary {
    pub name: String,
    pub default_branch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<ObjectId>,
    pub created_ms: i64,
    #[serde(default)]
    pub branches: usize,
    #[serde(default)]
    pub tags: usize,
    #[serde(default)]
    pub objects: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRepo {
    pub name: String,
    #[serde(default)]
    pub default_branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RefEntry {
    pub name: String,
    pub id: ObjectId,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BranchEntry {
    pub name: String,
    pub id: ObjectId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ahead: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behind: Option<usize>,
}

/// `{"any":true}` | `{"absent":true}` | `{"is":"<id>"}`
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ExpectSpec {
    Any(bool),
    Absent(bool),
    Is(ObjectId),
}

impl ExpectSpec {
    pub fn from_old(old: Option<ObjectId>) -> Self {
        match old {
            Some(id) => ExpectSpec::Is(id),
            None => ExpectSpec::Absent(true),
        }
    }
    pub fn to_core(self) -> agit_core::refs::Expect {
        match self {
            ExpectSpec::Any(_) => agit_core::refs::Expect::Any,
            ExpectSpec::Absent(_) => agit_core::refs::Expect::Absent,
            ExpectSpec::Is(id) => agit_core::refs::Expect::Is(id),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefUpdate {
    pub name: String,
    pub expect: ExpectSpec,
    pub new: Option<ObjectId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefUpdateRequest {
    pub updates: Vec<RefUpdate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefUpdateResponse {
    pub updated: Vec<RefEntry>,
    pub deleted: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdList {
    pub ids: Vec<ObjectId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissingResponse {
    pub missing: Vec<ObjectId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreResponse {
    pub stored: usize,
    pub skipped: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchRequest {
    pub wants: Vec<ObjectId>,
    #[serde(default)]
    pub haves: Vec<ObjectId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitEntry {
    pub id: ObjectId,
    #[serde(flatten)]
    pub commit: Commit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitDetail {
    pub id: ObjectId,
    pub commit: Commit,
    pub changes: Vec<Change>,
    pub patch: String,
    /// Signature status plus whether the key is registered for the signer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<SignatureInfo>,
    /// Notes linking this commit or covering a path it changed (at most 10).
    #[serde(default)]
    pub notes: Vec<NoteSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignatureInfo {
    #[serde(flatten)]
    pub status: agit_core::sign::SignatureStatus,
    /// True when the key is registered for the signer in the repository.
    #[serde(default)]
    pub trusted: bool,
}

/// A public signing key registered for an agent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KeyEntry {
    pub agent: String,
    pub key_id: String,
    pub public_key: String,
    pub added_ms: i64,
    #[serde(default)]
    pub added_by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyCreate {
    pub agent: String,
    pub public_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffResponse {
    pub changes: Vec<Change>,
    pub patch: String,
    pub stats: Vec<FileStat>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileStat {
    pub path: String,
    pub added: usize,
    pub removed: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum TreeResponse {
    Tree {
        path: String,
        id: ObjectId,
        entries: Vec<TreeEntryJson>,
    },
    Blob {
        path: String,
        id: ObjectId,
        size: u64,
        binary: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeEntryJson {
    pub name: String,
    pub mode: agit_core::EntryMode,
    pub id: ObjectId,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaseCreate {
    pub holder: String,
    pub paths: Vec<String>,
    #[serde(default)]
    pub mode: LeaseMode,
    #[serde(default)]
    pub ttl_secs: Option<u64>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaseRenew {
    pub ttl_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaseCheck {
    pub holder: String,
    pub paths: Vec<String>,
}

pub type LeaseJson = Lease;

// ---- pull requests ---------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PullState {
    Open,
    Merged,
    Closed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
    Approve,
    RequestChanges,
    Comment,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum CommentKind {
    Question,
    Suggestion,
    Blocker,
    #[default]
    Note,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Pending,
    Success,
    Failure,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Actor {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Review {
    pub reviewer: String,
    pub decision: ReviewDecision,
    #[serde(default)]
    pub body: String,
    pub time_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Comment {
    pub id: u64,
    pub author: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default)]
    pub kind: CommentKind,
    pub body: String,
    #[serde(default)]
    pub resolved: bool,
    pub time_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Check {
    pub name: String,
    pub status: CheckStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub time_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    pub source: String,
    pub target: String,
    pub source_id: ObjectId,
    pub target_id: ObjectId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_id: Option<ObjectId>,
    pub author: Actor,
    pub state: PullState,
    pub created_ms: i64,
    pub updated_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_commit: Option<ObjectId>,
    #[serde(default)]
    pub reviews: Vec<Review>,
    #[serde(default)]
    pub comments: Vec<Comment>,
    #[serde(default)]
    pub checks: Vec<Check>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullCreate {
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub intent: Option<String>,
    #[serde(default)]
    pub task: Option<String>,
    pub source: String,
    #[serde(default)]
    pub target: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PullUpdate {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub intent: Option<String>,
    #[serde(default)]
    pub task: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullDetail {
    pub pr: PullRequest,
    pub commits: Vec<CommitEntry>,
    pub changes: Vec<Change>,
    pub mergeable: bool,
    #[serde(default)]
    pub conflicts: Vec<Conflict>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommentCreate {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub line: Option<u32>,
    #[serde(default)]
    pub kind: CommentKind,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewCreate {
    pub decision: ReviewDecision,
    #[serde(default)]
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckCreate {
    pub name: String,
    pub status: CheckStatus,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum MergeStrategy {
    #[default]
    Merge,
    Ff,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MergeRequest {
    #[serde(default)]
    pub strategy: MergeStrategy,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeResponse {
    pub pr: PullRequest,
    pub merged_commit: ObjectId,
}

// ---- knowledge -------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub commits: Vec<ObjectId>,
    #[serde(default)]
    pub pulls: Vec<u64>,
    pub author: String,
    #[serde(default = "default_confidence")]
    pub confidence: f32,
    pub created_ms: i64,
    pub updated_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
    /// Commit the note was written or last validated against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_at: Option<ObjectId>,
    /// Drafts are machine-generated (e.g. from a merged pull request) and
    /// hidden from listings until accepted.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub draft: bool,
    /// Computed on read: a path covered by `paths` changed between `valid_at` and HEAD.
    #[serde(default)]
    pub stale: bool,
}

fn default_confidence() -> f32 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteSummary {
    pub id: String,
    pub title: String,
    pub tags: Vec<String>,
    pub paths: Vec<String>,
    pub author: String,
    pub confidence: f32,
    pub updated_ms: i64,
    pub excerpt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_at: Option<ObjectId>,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub stale: bool,
    /// BM25 relevance; present only for full-text queries (`q=`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
}

impl From<&Note> for NoteSummary {
    fn from(n: &Note) -> Self {
        let excerpt: String = n.body.chars().take(200).collect();
        NoteSummary {
            id: n.id.clone(),
            title: n.title.clone(),
            tags: n.tags.clone(),
            paths: n.paths.clone(),
            author: n.author.clone(),
            confidence: n.confidence,
            updated_ms: n.updated_ms,
            excerpt,
            superseded_by: n.superseded_by.clone(),
            valid_at: n.valid_at,
            draft: n.draft,
            stale: n.stale,
            score: None,
        }
    }
}

/// A stale note together with the covered paths that changed since `valid_at`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StaleNote {
    #[serde(flatten)]
    pub note: NoteSummary,
    pub changed_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ValidateRequest {
    /// Revision to validate against (default HEAD).
    #[serde(default)]
    pub rev: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NoteCreate {
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub commits: Vec<ObjectId>,
    #[serde(default)]
    pub pulls: Vec<u64>,
    #[serde(default)]
    pub confidence: Option<f32>,
    /// Defaults to HEAD of the default branch when it exists.
    #[serde(default)]
    pub valid_at: Option<ObjectId>,
    #[serde(default)]
    pub draft: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NotePatch {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub paths: Option<Vec<String>>,
    #[serde(default)]
    pub commits: Option<Vec<ObjectId>>,
    #[serde(default)]
    pub pulls: Option<Vec<u64>>,
    #[serde(default)]
    pub confidence: Option<f32>,
    #[serde(default)]
    pub superseded_by: Option<Option<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagCount {
    pub tag: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TagRule {
    pub pattern: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextResponse {
    pub path: String,
    /// Fresh notes first, stale notes (`stale: true`) after them.
    pub notes: Vec<NoteSummary>,
    pub suggested_tags: Vec<String>,
    pub recent_commits: Vec<CommitEntry>,
    pub leases: Vec<Lease>,
    pub open_pulls: Vec<PullRequest>,
    /// Agents holding a lease on the path or who committed to it in the last 24h.
    #[serde(default)]
    pub agents_active: Vec<String>,
    /// File contents (only with `include=files`), trimmed to the token budget.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<ContextFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextFile {
    pub path: String,
    pub text: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Webhook {
    pub id: String,
    pub url: String,
    /// Event kinds to deliver: exact (`commit`), prefix (`pull.*`), or empty for all.
    #[serde(default)]
    pub kinds: Vec<String>,
    #[serde(default)]
    pub active: bool,
    /// Whether a signing secret is configured (the secret itself is never returned).
    #[serde(default)]
    pub has_secret: bool,
    pub created_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebhookCreate {
    pub url: String,
    #[serde(default)]
    pub kinds: Vec<String>,
    #[serde(default)]
    pub secret: Option<String>,
}

/// Repository policy, enforced server-side at ref updates and PR merges.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Policy {
    /// Full ref names (e.g. `refs/heads/main`) or branch globs (`release/*`).
    #[serde(default)]
    pub protected_branches: Vec<String>,
    /// Protected branches only move through pull request merges.
    #[serde(default)]
    pub require_pull_request: bool,
    /// Minimum number of distinct approving reviewers (excluding the PR author) to merge into a protected branch.
    #[serde(default)]
    pub require_approvals: u32,
    /// Every commit pushed to a protected branch must carry `intent`.
    #[serde(default)]
    pub require_intent: bool,
    /// Every commit pushed to a protected branch must carry an `agent` identity.
    #[serde(default)]
    pub require_agent: bool,
    /// All checks must be `success` (and at least one must exist) before merging a PR into a protected branch.
    #[serde(default)]
    pub require_checks: bool,
    /// Unresolved `blocker` comments prevent merging.
    #[serde(default = "default_true")]
    pub block_on_blockers: bool,
    /// Only these agent ids may merge into protected branches (empty = anyone).
    #[serde(default)]
    pub allowed_merge_agents: Vec<String>,
    /// Only these agent ids may push directly to protected branches (empty = anyone, subject to require_pull_request).
    #[serde(default)]
    pub allowed_push_agents: Vec<String>,
    /// Reject commits touching more than this many files (0 = unlimited).
    #[serde(default)]
    pub max_files_per_commit: u32,
    /// Every commit reaching a protected branch must carry a valid Ed25519
    /// signature whose key is registered for the signer (`/keys`).
    #[serde(default)]
    pub require_signed: bool,
}

fn default_true() -> bool {
    true
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            protected_branches: vec![],
            require_pull_request: false,
            require_approvals: 0,
            require_intent: false,
            require_agent: false,
            require_checks: false,
            block_on_blockers: true,
            allowed_merge_agents: vec![],
            allowed_push_agents: vec![],
            max_files_per_commit: 0,
            require_signed: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentActivity {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub commits: usize,
    pub last_seen_ms: i64,
    pub open_pulls: usize,
    pub leases: usize,
}
