//! # agit-core
//!
//! Core library of **agit**, a content-addressed version control system with
//! git's workflow and an object model, concurrency model and output contract
//! designed for AI agents working in parallel.
//!
//! Start with [`Repository`] for discovery and layout, then [`ops`] for the
//! high-level operations the CLI exposes (commit, log, merge, ...).

pub mod diff;
pub mod durability;
pub mod error;
pub mod events;
pub mod hash;
pub mod index;
pub mod lease;
pub mod lock;
pub mod merge;
pub mod object;
pub mod ops;
pub mod refs;
pub mod repo;
pub mod store;
pub mod submodule;
pub mod worktree;

pub use error::{Error, Result};
pub use hash::ObjectId;
pub use object::{AgentInfo, Commit, EntryMode, Object, ObjectKind, Signature, Tree, TreeEntry};
pub use repo::{Head, Repository};

/// Version of the on-disk format written by this library.
pub const FORMAT_VERSION: u32 = repo::FORMAT_VERSION;
