//! Shared wire types and helpers for the agit server, CLI and SDKs.
//!
//! See `docs/API.md` for the HTTP contract this crate implements.

pub mod client;
pub mod stream;
pub mod sync;
pub mod types;
pub mod walk;

pub use client::Client;
