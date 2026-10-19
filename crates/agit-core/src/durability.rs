//! Process-wide durability policy (`AGIT_FSYNC`).
//!
//! Like git, agit does not fsync every object by default: objects are
//! written atomically (temp + rename, or an append to a pack segment that
//! is only referenced after its index is published), so a crash can never
//! leave a torn object, and the reference update that makes them reachable
//! is what gets synced. The levels differ in *how* that sync is done:
//!
//! * `refs` (default): the ref journal, HEAD and other metadata are synced
//!   with `fsync(2)`, exactly what git does. On Linux that reaches stable
//!   storage. On macOS `fsync(2)` hands the data to the drive without
//!   flushing its write cache (git's default there too): a process or kernel
//!   crash is survived, a sudden power loss may lose the last commits.
//! * `full`: like `refs`, with a full flush to stable storage (`F_FULLFSYNC`
//!   on macOS, `fdatasync` elsewhere). Power-loss safe, several times slower
//!   per commit on Apple hardware.
//! * `all`: `full`, plus every object, index and log write is flushed.
//! * `none`: never fsync (throwaway repositories and CI only).

use std::fs::File;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Durability {
    /// Full flush of refs, journal, objects, index and logs.
    All = 0,
    /// `fsync(2)` of refs and journal only (default; matches git's defaults).
    Refs = 1,
    /// Never fsync.
    None = 2,
    /// Full flush of refs and journal; objects and logs as in `refs`.
    Full = 3,
}

static LEVEL: AtomicU8 = AtomicU8::new(1);
static INIT: OnceLock<()> = OnceLock::new();

fn init_from_env() {
    INIT.get_or_init(|| {
        if let Ok(v) = std::env::var("AGIT_FSYNC") {
            let lvl = match v.to_ascii_lowercase().as_str() {
                "all" => Durability::All,
                "full" => Durability::Full,
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
        3 => Durability::Full,
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

/// Sync `file` the way the current level asks for: a plain `fsync(2)` at
/// `refs`, a full flush to stable storage at `full` and `all`, nothing at
/// `none`. Every fsync in agit goes through here.
pub fn sync_file(file: &File) -> std::io::Result<()> {
    match get() {
        Durability::None => Ok(()),
        Durability::Refs => plain_fsync(file),
        Durability::Full | Durability::All => file.sync_data(),
    }
}

/// `fsync(2)` without the drive-cache flush that the standard library adds
/// on macOS (`F_FULLFSYNC`); elsewhere the standard call already is the
/// plain one.
#[cfg(target_os = "macos")]
fn plain_fsync(file: &File) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    extern "C" {
        fn fsync(fd: std::os::raw::c_int) -> std::os::raw::c_int;
    }
    if unsafe { fsync(file.as_raw_fd()) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "macos"))]
fn plain_fsync(file: &File) -> std::io::Result<()> {
    file.sync_data()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_and_sync() {
        let before = get();
        for lvl in [
            Durability::None,
            Durability::Refs,
            Durability::Full,
            Durability::All,
        ] {
            set(lvl);
            assert_eq!(get(), lvl);
            let f = tempfile::tempfile().unwrap();
            sync_file(&f).unwrap();
        }
        assert!(sync_data());
        set(Durability::Refs);
        assert!(sync_refs() && !sync_data());
        set(before);
    }
}
