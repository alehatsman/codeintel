//! The writer lock: `.codeintel/lock`, advisory, exclusive, non-blocking.
//!
//! One writer, many readers (`specs/04-storage.md` § Concurrency). Two
//! concurrent writers are undefined behaviour, so a second one fails rather
//! than racing — `dict.bin` is appended in place with no tmp-and-rename, and
//! interleaved appends put `dict.idx` out of step with it, after which every
//! atom past that point resolves to the wrong string with no checksum to
//! notice.
//!
//! `query`'s auto-refresh is a writer and takes this same lock. A *reader*
//! that cannot take it does not fail: it reads the current manifest and
//! reports `stale`. `locked` is for a second writer, and telling a reader
//! "locked" is not actionable.

use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Result, Seek, SeekFrom, Write};
use std::path::Path;

/// The lock file name, inside `.codeintel/`.
pub const LOCK: &str = "lock";

/// An openable lock. Hold the [`Held`] it hands out for as long as you write.
#[derive(Debug)]
pub struct Lock {
    path: std::path::PathBuf,
    inner: fd_lock::RwLock<File>,
}

/// Proof that this process holds the writer lock. Releases on drop.
#[derive(Debug)]
pub struct Held<'a> {
    _guard: fd_lock::RwLockWriteGuard<'a, File>,
}

impl Lock {
    /// Open (creating if needed) `dir/lock`, without taking it.
    ///
    /// # Errors
    /// I/O failure creating the directory or the file.
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join(LOCK))?;
        Ok(Self {
            path: dir.join(LOCK),
            inner: fd_lock::RwLock::new(file),
        })
    }

    /// Take it, or report who holds it.
    ///
    /// The holder's pid is recorded in the file so `status: "locked"` can name
    /// a process instead of a condition.
    ///
    /// # Errors
    /// I/O failure other than contention, which is [`Contended`] instead.
    pub fn try_hold(&mut self) -> Result<std::result::Result<Held<'_>, Contended>> {
        // Cloned before the borrow: the guard borrows `self` for the returned
        // lifetime, so the contended arm cannot reach back through `self`.
        let path = self.path.clone();
        match self.inner.try_write() {
            Ok(mut guard) => {
                guard.seek(SeekFrom::Start(0))?;
                guard.set_len(0)?;
                write!(guard, "{}", std::process::id())?;
                guard.flush()?;
                Ok(Ok(Held { _guard: guard }))
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                Ok(Err(Contended { pid: pid_in(&path) }))
            }
            Err(e) => Err(e),
        }
    }
}

/// The pid recorded by whoever holds the lock, when it is readable.
///
/// Read through a fresh handle and without taking any lock: this runs on the
/// contended path, where the point is to name the holder, not to wait for it.
/// A missing or garbled pid is `None` rather than an error — the status is
/// `locked` either way.
fn pid_in(path: &Path) -> Option<u32> {
    let mut text = String::new();
    File::open(path).ok()?.read_to_string(&mut text).ok()?;
    text.trim().parse().ok()
}

/// Someone else is writing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Contended {
    /// The holder's pid, when the lock file was readable.
    pub pid: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_writer_is_refused_and_a_release_lets_it_in() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut first = Lock::open(dir.path()).expect("opens");
        let mut second = Lock::open(dir.path()).expect("opens");

        let held = first.try_hold().expect("no io error").expect("uncontended");
        assert!(
            second.try_hold().expect("no io error").is_err(),
            "two writers took the same lock"
        );
        drop(held);
        assert!(
            second.try_hold().expect("no io error").is_ok(),
            "the lock was not released"
        );
    }

    #[test]
    fn holding_records_a_pid() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut lock = Lock::open(dir.path()).expect("opens");
        let held = lock.try_hold().expect("no io error").expect("uncontended");
        let text = std::fs::read_to_string(dir.path().join(LOCK)).expect("readable");
        assert_eq!(text.trim().parse::<u32>().ok(), Some(std::process::id()));
        drop(held);
    }
}
