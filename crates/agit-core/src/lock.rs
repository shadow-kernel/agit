//! Cross-process locking.
//!
//! We use advisory `flock` locks instead of git-style `*.lock` sentinel
//! files. A crashed agent therefore never leaves a stale lock behind that a
//! human has to clean up by hand, which is the single most common way
//! concurrent git usage breaks.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fs2::FileExt;

use crate::error::{io_at, Error, Result};

pub struct Lock {
    _file: File,
    path: PathBuf,
}

impl Lock {
    /// Acquire an exclusive lock, waiting up to `timeout`.
    pub fn acquire(path: &Path, timeout: Duration) -> Result<Lock> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io_at(parent))?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .map_err(io_at(path))?;
        let start = Instant::now();
        let mut backoff = Duration::from_millis(1);
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => {
                    return Ok(Lock {
                        _file: file,
                        path: path.to_path_buf(),
                    })
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || is_lock_contention(&e) => {
                    if start.elapsed() >= timeout {
                        return Err(Error::LockTimeout(path.to_path_buf()));
                    }
                    std::thread::sleep(backoff);
                    backoff = (backoff * 2).min(Duration::from_millis(50));
                }
                Err(e) => {
                    return Err(Error::IoAt {
                        path: path.to_path_buf(),
                        source: e,
                    })
                }
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn is_lock_contention(e: &std::io::Error) -> bool {
    // fs2 reports contention as the platform's EWOULDBLOCK/EAGAIN.
    matches!(e.raw_os_error(), Some(11) | Some(35))
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = self._file.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_lock_times_out_while_first_held() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.lock");
        let _a = Lock::acquire(&p, Duration::from_millis(100)).unwrap();
        let b = Lock::acquire(&p, Duration::from_millis(50));
        assert!(matches!(b, Err(Error::LockTimeout(_))));
    }

    #[test]
    fn lock_released_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.lock");
        {
            let _a = Lock::acquire(&p, Duration::from_millis(100)).unwrap();
        }
        Lock::acquire(&p, Duration::from_millis(50)).unwrap();
    }
}
