//! Process-wide durability policy.
//!
//! Like git, agit does not fsync every loose object by default: objects are
//! written atomically (temp + rename) so a crash can never leave a torn
//! object, and the reference update that makes them reachable is always
//! synced. `AGIT_FSYNC=all` forces fsync on every write; `AGIT_FSYNC=none`
//! disables it entirely (useful for throwaway CI repositories).

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Durability {
    /// fsync objects, refs and logs.
    All = 0,
    /// fsync refs only (default; matches git's defaults).
    Refs = 1,
    /// never fsync.
    None = 2,
}

static LEVEL: AtomicU8 = AtomicU8::new(1);
static INIT: OnceLock<()> = OnceLock::new();

fn init_from_env() {
    INIT.get_or_init(|| {
        if let Ok(v) = std::env::var("AGIT_FSYNC") {
            let lvl = match v.to_ascii_lowercase().as_str() {
                "all" | "full" => Durability::All,
                "none" | "off" => Durability::None,
                _ => Durability::Refs,
            };
            LEVEL.store(lvl as u8, Ordering::Relaxed);
        }
    });
}

pub fn set(level: Durability) {
    INIT.get_or_init(|| ());
    LEVEL.store(level as u8, Ordering::Relaxed);
}

pub fn get() -> Durability {
    init_from_env();
    match LEVEL.load(Ordering::Relaxed) {
        0 => Durability::All,
        2 => Durability::None,
        _ => Durability::Refs,
    }
}

/// Should bulk data (objects, index, logs) be fsynced?
pub fn sync_data() -> bool {
    get() == Durability::All
}

/// Should reference updates be fsynced?
pub fn sync_refs() -> bool {
    get() != Durability::None
}
