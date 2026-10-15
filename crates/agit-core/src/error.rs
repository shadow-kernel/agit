use std::path::PathBuf;

use crate::hash::ObjectId;

/// Every error carries a stable machine-readable `code()` so agents can
/// branch on failures without parsing human prose.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not an agit repository (or any parent): {0}")]
    NotARepository(PathBuf),

    #[error("repository already exists at {0}")]
    AlreadyExists(PathBuf),

    #[error("object not found: {0}")]
    ObjectNotFound(ObjectId),

    #[error("object {id} is corrupt: {reason}")]
    CorruptObject { id: ObjectId, reason: String },

    #[error("object {expected} failed verification (content hashes to {actual})")]
    HashMismatch {
        expected: ObjectId,
        actual: ObjectId,
    },

    #[error("reference not found: {0}")]
    RefNotFound(String),

    #[error("invalid reference name: {0}")]
    InvalidRefName(String),

    #[error("reference {name} changed concurrently (expected {expected:?}, found {actual:?})")]
    RefConflict {
        name: String,
        expected: Option<ObjectId>,
        actual: Option<ObjectId>,
    },

    #[error("invalid object id: {0}")]
    InvalidObjectId(String),

    #[error("ambiguous revision: {0}")]
    AmbiguousRevision(String),

    #[error("unknown revision: {0}")]
    UnknownRevision(String),

    #[error("path is outside the working tree: {0}")]
    PathOutsideWorktree(PathBuf),

    #[error("path not tracked: {0}")]
    PathNotTracked(PathBuf),

    #[error("nothing to commit")]
    NothingToCommit,

    #[error("working tree has uncommitted changes in: {0:?}")]
    DirtyWorktree(Vec<PathBuf>),

    #[error("merge produced conflicts in {0} file(s)")]
    MergeConflicts(usize),

    #[error("lease conflict: {path} is held by {holder} (lease {lease_id})")]
    LeaseConflict {
        path: String,
        holder: String,
        lease_id: String,
    },

    #[error("lease not found: {0}")]
    LeaseNotFound(String),

    #[error("workspace already exists: {0}")]
    WorkspaceExists(String),

    #[error("workspace not found: {0}")]
    WorkspaceNotFound(String),

    #[error("lock timeout: {0}")]
    LockTimeout(PathBuf),

    #[error("policy violation: {0}")]
    PolicyViolation(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("forbidden: {0}")]
    Forbidden(String),

    #[error("non-fast-forward: remote {remote} is not an ancestor of local {local}; pull first or push with --force")]
    NonFastForward { remote: ObjectId, local: ObjectId },

    #[error("{0}")]
    Invalid(String),

    #[error("io error at {path}: {source}")]
    IoAt {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serde(String),
}

impl Error {
    /// Stable identifier for programmatic error handling (`agit --json`).
    pub fn code(&self) -> &'static str {
        use Error::*;
        match self {
            NotARepository(_) => "not_a_repository",
            AlreadyExists(_) => "already_exists",
            ObjectNotFound(_) => "object_not_found",
            CorruptObject { .. } => "corrupt_object",
            HashMismatch { .. } => "hash_mismatch",
            RefNotFound(_) => "ref_not_found",
            InvalidRefName(_) => "invalid_ref_name",
            RefConflict { .. } => "ref_conflict",
            InvalidObjectId(_) => "invalid_object_id",
            AmbiguousRevision(_) => "ambiguous_revision",
            UnknownRevision(_) => "unknown_revision",
            PathOutsideWorktree(_) => "path_outside_worktree",
            PathNotTracked(_) => "path_not_tracked",
            NothingToCommit => "nothing_to_commit",
            DirtyWorktree(_) => "dirty_worktree",
            MergeConflicts(_) => "merge_conflicts",
            LeaseConflict { .. } => "lease_conflict",
            LeaseNotFound(_) => "lease_not_found",
            WorkspaceExists(_) => "workspace_exists",
            WorkspaceNotFound(_) => "workspace_not_found",
            LockTimeout(_) => "lock_timeout",
            PolicyViolation(_) => "policy_violation",
            NotFound(_) => "not_found",
            Forbidden(_) => "forbidden",
            NonFastForward { .. } => "non_fast_forward",
            Invalid(_) => "invalid",
            IoAt { .. } | Io(_) => "io",
            Serde(_) => "serde",
        }
    }

    /// Structured details for machine consumers (same shape as the server).
    pub fn details(&self) -> Option<serde_json::Value> {
        use Error::*;
        match self {
            RefConflict {
                name,
                expected,
                actual,
            } => Some(serde_json::json!({ "name": name, "expected": expected, "actual": actual })),
            LeaseConflict {
                path,
                holder,
                lease_id,
            } => Some(serde_json::json!({ "path": path, "holder": holder, "lease_id": lease_id })),
            MergeConflicts(n) => Some(serde_json::json!({ "count": n })),
            NonFastForward { remote, local } => {
                Some(serde_json::json!({ "remote": remote, "local": local }))
            }
            DirtyWorktree(paths) => Some(serde_json::json!({ "paths": paths })),
            ObjectNotFound(id) => Some(serde_json::json!({ "id": id })),
            _ => None,
        }
    }

    /// Whether an agent may sensibly retry the same operation.
    pub fn is_retryable(&self) -> bool {
        matches!(self, Error::RefConflict { .. } | Error::LockTimeout(_))
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Serde(e.to_string())
    }
}

impl From<bincode::Error> for Error {
    fn from(e: bincode::Error) -> Self {
        Error::Serde(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn io_at<P: Into<PathBuf>>(path: P) -> impl FnOnce(std::io::Error) -> Error {
    let path = path.into();
    move |source| Error::IoAt { path, source }
}
