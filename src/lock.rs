//! Advisory file-lock for all persisted mind state.
//!
//! A single lock file at `<mind root>/.lock` serializes mutations and protects
//! concurrent readers from observing partial writes. The lock is advisory:
//! it constrains only mind processes that call this module.
//!
//! # Usage
//!
//! In `run`, right after `Paths::resolve`:
//!
//! ```ignore
//! let mut lock = lock::open(&paths)?;
//! let _guard = lock.write()?;    // exclusive, or lock.read()? for shared
//! // ... rest of command ...
//! // _guard dropped here, lock released
//! ```

use std::fs::{File, OpenOptions};

use fd_lock::RwLock;

use crate::error::{MindError, Result};
use crate::paths::{Paths, mkdir_p};

/// An opened lock file, ready to be locked shared or exclusively.
///
/// Keep this value alive for the duration you want the lock held; acquire a
/// guard via [`MindLock::write`] or [`MindLock::read`] and drop the guard to
/// release the OS lock.
pub struct MindLock {
    inner: RwLock<File>,
    /// Remembered so error messages carry the path.
    path: std::path::PathBuf,
}

/// RAII exclusive guard. Holds the OS write lock until dropped.
pub struct WriteGuard<'a> {
    _guard: fd_lock::RwLockWriteGuard<'a, File>,
}

/// RAII shared guard. Holds the OS read lock until dropped.
pub struct ReadGuard<'a> {
    _guard: fd_lock::RwLockReadGuard<'a, File>,
}

/// Open (creating if necessary) the lock file for the given paths.
///
/// This does not acquire the lock yet; call [`MindLock::write`] or
/// [`MindLock::read`] on the returned value to block until the lock is held.
pub fn open(paths: &Paths) -> Result<MindLock> {
    // The mind home must exist before we can create the lock file.
    mkdir_p(&paths.mind_home)?;
    let lock_path = paths.lock_file();
    let file = OpenOptions::new()
        .create(true)
        .truncate(false) // lock token only; preserve existing content
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|e| MindError::io(&lock_path, e))?;
    Ok(MindLock {
        inner: RwLock::new(file),
        path: lock_path,
    })
}

impl MindLock {
    /// Acquire the lock exclusively (mutating commands). Blocks until available.
    // spec: STO-40 STO-41 STO-42
    pub fn write(&mut self) -> Result<WriteGuard<'_>> {
        let guard = self
            .inner
            .write()
            .map_err(|e| MindError::io(&self.path, e))?;
        Ok(WriteGuard { _guard: guard })
    }

    /// Acquire the lock shared (read-only commands). Blocks until available.
    // spec: STO-40 STO-41 STO-42
    pub fn read(&self) -> Result<ReadGuard<'_>> {
        let guard = self
            .inner
            .read()
            .map_err(|e| MindError::io(&self.path, e))?;
        Ok(ReadGuard { _guard: guard })
    }

    /// Try to acquire a shared lock without blocking. Returns `None` if the
    /// lock is already held exclusively by another process (e.g. during a TUI
    /// poll tick while a mutation is in progress). The TUI uses this for its
    /// ~1s refresh poll so it never freezes behind a writer (TUI-15, TUI-25).
    // spec: TUI-25
    pub fn try_read(&self) -> Option<ReadGuard<'_>> {
        self.inner.try_read().ok().map(|g| ReadGuard { _guard: g })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::MindError;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_paths(label: &str) -> (Paths, std::path::PathBuf) {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base =
            std::env::temp_dir().join(format!("mind-lock-{}-{n}-{label}", std::process::id()));
        let paths = Paths {
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        };
        (paths, base)
    }

    fn cleanup(base: &std::path::Path) {
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn exclusive_lock_on_fresh_home_succeeds() {
        // spec: STO-42
        let (paths, base) = temp_paths("excl");
        let mut lock = open(&paths).expect("open lock on fresh mind home");
        let _guard = lock.write().expect("exclusive acquire should succeed");
        drop(_guard);
        cleanup(&base);
    }

    #[test]
    fn shared_lock_on_fresh_home_succeeds() {
        // spec: STO-42
        let (paths, base) = temp_paths("shared");
        let lock = open(&paths).expect("open lock on fresh mind home");
        let _guard = lock.read().expect("shared acquire should succeed");
        drop(_guard);
        cleanup(&base);
    }

    #[test]
    fn two_shared_locks_coexist() {
        // Two separate RwLock handles on the same file can both hold a read lock.
        // spec: STO-41 STO-42
        let (paths, base) = temp_paths("twoshared");
        // Ensure the mind home and lock file exist first.
        mkdir_p(&paths.mind_home).unwrap();
        let lock_path = paths.lock_file();

        let f1 = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();
        let f2 = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();

        let l1 = RwLock::new(f1);
        let l2 = RwLock::new(f2);

        // Both shared guards held at the same time.
        let _g1 = l1.read().expect("first shared lock");
        // A second shared lock must not block - use try_read to avoid blocking.
        let _g2 = l2
            .try_read()
            .expect("second shared lock should succeed while first is held");
        cleanup(&base);
    }

    #[test]
    fn exclusive_lock_excludes_a_second_exclusive_holder() {
        // Hold an exclusive lock on one handle; a second handle's try_write must
        // be refused while the first is held, then succeed once it is dropped.
        // This is the core mutual-exclusion guarantee; it would fail if write()
        // did not take a real OS-level exclusive lock.
        // spec: STO-41 STO-42
        let (paths, base) = temp_paths("exclexcl");
        mkdir_p(&paths.mind_home).unwrap();
        let lock_path = paths.lock_file();

        let f1 = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();
        let f2 = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();
        let mut l1 = RwLock::new(f1);
        let mut l2 = RwLock::new(f2);

        let g1 = l1.write().expect("first exclusive lock");
        assert!(
            l2.try_write().is_err(),
            "a second exclusive lock must be refused while the first is held"
        );
        drop(g1);
        // After release the second exclusive lock must now succeed.
        let _g2 = l2
            .try_write()
            .expect("second exclusive lock should succeed after the first is released");
        cleanup(&base);
    }

    #[test]
    fn exclusive_lock_excludes_a_shared_reader() {
        // A held exclusive lock must block shared readers too: a reader must not
        // observe a writer mid-update. try_read must be refused while the
        // exclusive lock is held.
        // spec: STO-41 STO-42
        let (paths, base) = temp_paths("exclshared");
        mkdir_p(&paths.mind_home).unwrap();
        let lock_path = paths.lock_file();

        let f1 = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();
        let f2 = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();
        let mut l1 = RwLock::new(f1);
        let l2 = RwLock::new(f2);

        let g1 = l1.write().expect("exclusive lock");
        assert!(
            l2.try_read().is_err(),
            "a shared reader must be refused while an exclusive lock is held"
        );
        drop(g1);
        let _g2 = l2
            .try_read()
            .expect("shared read should succeed after the exclusive lock is released");
        cleanup(&base);
    }

    #[test]
    fn shared_lock_excludes_an_exclusive_writer() {
        // A held shared lock must block a would-be exclusive writer: the writer
        // cannot begin a mutation while a reader holds the snapshot.
        // spec: STO-41 STO-42
        let (paths, base) = temp_paths("sharedexcl");
        mkdir_p(&paths.mind_home).unwrap();
        let lock_path = paths.lock_file();

        let f1 = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();
        let f2 = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();
        let l1 = RwLock::new(f1);
        let mut l2 = RwLock::new(f2);

        let g1 = l1.read().expect("shared lock");
        assert!(
            l2.try_write().is_err(),
            "an exclusive writer must be refused while a shared lock is held"
        );
        drop(g1);
        let _g2 = l2
            .try_write()
            .expect("exclusive write should succeed after the shared lock is released");
        cleanup(&base);
    }

    #[test]
    fn exclusive_write_blocks_until_holder_releases() {
        // End-to-end through the public API (open + write): a second exclusive
        // acquisition must BLOCK (not error, not proceed) until the first is
        // released. We prove blocking by ordering: thread B records the time it
        // acquires, and must acquire only after thread A has held the lock for a
        // measurable interval. A non-locking implementation would let B proceed
        // immediately and the ordering invariant would fail.
        // spec: STO-42
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;
        use std::time::{Duration, Instant};

        let (paths, base) = temp_paths("blockwait");
        mkdir_p(&paths.mind_home).unwrap();
        let paths = Arc::new(paths);
        let a_acquired = Arc::new(AtomicBool::new(false));
        let a_released = Arc::new(AtomicBool::new(false));

        let hold = Duration::from_millis(300);
        let p_a = Arc::clone(&paths);
        let acq_a = Arc::clone(&a_acquired);
        let rel_a = Arc::clone(&a_released);
        let a = std::thread::spawn(move || {
            let mut lock = open(&p_a).expect("open A");
            let guard = lock.write().expect("A exclusive");
            // Signal acquisition before the hold so B contends for real. A fixed
            // head-start sleep is unreliable on a loaded runner: it can oversleep
            // past the whole hold, so B starts after A already released.
            acq_a.store(true, Ordering::SeqCst);
            // Hold long enough that B must observe the release ordering.
            std::thread::sleep(hold);
            rel_a.store(true, Ordering::SeqCst);
            drop(guard);
        });

        // Wait until A actually holds the exclusive lock.
        while !a_acquired.load(Ordering::SeqCst) {
            std::thread::yield_now();
        }

        let p_b = Arc::clone(&paths);
        let rel_b = Arc::clone(&a_released);
        let start = Instant::now();
        let b = std::thread::spawn(move || {
            let mut lock = open(&p_b).expect("open B");
            let _guard = lock.write().expect("B exclusive");
            // When B finally acquires, A must already have released.
            assert!(
                rel_b.load(Ordering::SeqCst),
                "B acquired the exclusive lock before A released it (no mutual exclusion)"
            );
            start.elapsed()
        });

        a.join().unwrap();
        let waited = b.join().unwrap();
        assert!(
            waited >= Duration::from_millis(200),
            "B should have blocked roughly until A released; only waited {waited:?}"
        );
        cleanup(&base);
    }

    #[test]
    fn lock_failure_is_io_error_with_lock_path() {
        // When we cannot open the lock file (e.g. the path is a directory),
        // the error must be MindError::Io carrying the lock file path.
        // spec: STO-42
        let (paths, base) = temp_paths("err");
        // Create the mind home directory first.
        mkdir_p(&paths.mind_home).unwrap();
        let lock_path = paths.lock_file();
        // Make the lock path a directory so opening it as a file fails.
        std::fs::create_dir_all(&lock_path).unwrap();

        let result = open(&paths);
        cleanup(&base);
        match result {
            Err(MindError::Io { path, .. }) => {
                assert_eq!(path, lock_path, "Io error should carry the lock path");
            }
            Ok(_) => panic!("expected an error when lock path is a directory"),
            Err(e) => panic!("unexpected error variant: {e:?}"),
        }
    }
}
