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
        }
    }
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
    pub notes: Vec<NoteSummary>,
    pub suggested_tags: Vec<String>,
    pub recent_commits: Vec<CommitEntry>,
    pub leases: Vec<Lease>,
    pub open_pulls: Vec<PullRequest>,
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
