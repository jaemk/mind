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
    /// One lock file per mind home, the default home first (HOME-9).
    files: Vec<OneLock>,
}

struct OneLock {
    inner: RwLock<File>,
    /// Remembered so error messages carry the path.
    path: std::path::PathBuf,
    /// Whether the lock file opened for writing, so this home can carry a
    /// holder record (STO-84).
    writable: bool,
}

impl OneLock {
    fn holder_path(&self) -> std::path::PathBuf {
        holder_path(&self.path)
    }
}

/// RAII exclusive guard. Holds the OS write locks until dropped.
pub struct WriteGuard<'a> {
    /// Holder records this guard wrote (STO-84). `Drop` removes them before
    /// the fields drop, so while the locks are still held.
    holders: Vec<std::path::PathBuf>,
    _guards: Vec<fd_lock::RwLockWriteGuard<'a, File>>,
}

impl Drop for WriteGuard<'_> {
    fn drop(&mut self) {
        for path in &self.holders {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// RAII shared guard. Holds the OS read locks until dropped.
pub struct ReadGuard<'a> {
    _guards: Vec<fd_lock::RwLockReadGuard<'a, File>>,
}

/// Open (creating if necessary) the lock file of every mind home.
///
/// This does not acquire the lock yet; call [`MindLock::write`] or
/// [`MindLock::read`] on the returned value to block until the lock is held.
///
/// spec: HOME-9 -- every allowed home has its own `.lock`, taken in a fixed
/// order (the default home, then `extra_homes` order) so two commands cannot
/// deadlock. A home whose lock file cannot be opened for writing (a sandbox
/// that allows only reads there) is locked through a read-only handle, and one
/// with no lock file that cannot be created because writes are denied is
/// skipped: nothing will be written there. Any other failure to open the
/// default home's lock is an error, as before.
pub fn open(paths: &Paths) -> Result<MindLock> {
    let mut files = Vec::new();
    if let Some(lock) = open_one(&paths.mind_home, paths.lock_file(), true)? {
        files.push(lock);
    }
    for home in paths.extra_homes() {
        if let Some(lock) = open_one(&home, home.join(".lock"), false)? {
            files.push(lock);
        }
    }
    Ok(MindLock { files })
}

fn open_one(
    home: &std::path::Path,
    lock_path: std::path::PathBuf,
    required: bool,
) -> Result<Option<OneLock>> {
    // The mind home must exist before we can create the lock file.
    let made = mkdir_p(home);
    let rw = OpenOptions::new()
        .create(true)
        .truncate(false) // lock token only; preserve existing content
        .read(true)
        .write(true)
        .open(&lock_path);
    // Only a home we may not write gets the read-only fallback; any other
    // failure (e.g. the lock path is a directory) is reported as before.
    let denied = |e: &std::io::Error| {
        matches!(
            e.kind(),
            std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem
        )
    };
    // A home that could not be created because writes are denied there is
    // read-only in the same sense as one whose lock file cannot be opened.
    let home_denied = matches!(&made, Err(MindError::Io { source, .. }) if denied(source));
    let (file, writable) = match rw {
        Ok(f) => (f, true),
        Err(e) if denied(&e) || home_denied => {
            match OpenOptions::new().read(true).open(&lock_path) {
                Ok(f) => (f, false),
                // Nothing can be written in this home, so there is nothing
                // for its lock to guard.
                Err(_) => return Ok(None),
            }
        }
        Err(_) if !required => return Ok(None),
        Err(e) => {
            made?;
            return Err(MindError::io(&lock_path, e));
        }
    };
    Ok(Some(OneLock {
        inner: RwLock::new(file),
        path: lock_path,
        writable,
    }))
}

/// The holder record beside a lock file: `<home>/.lock.holder` (STO-84).
fn holder_path(lock_path: &std::path::Path) -> std::path::PathBuf {
    lock_path.with_file_name(".lock.holder")
}

/// Who holds a home's exclusive lock (STO-84): written by the holder once it
/// has the lock and removed before it lets go, so a command that waits on the
/// lock can say what it waits on.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Holder {
    /// The holding process.
    pub pid: u32,
    /// The holder's command line, `mind <args>`, with any URL credential cut.
    pub command: String,
    /// The holder's working directory, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// When the holder took the lock, in Unix seconds. Defaulted, and unknown
    /// fields are ignored, so a record from another version still parses.
    #[serde(default)]
    pub started: u64,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Holder {
    /// The record for this process.
    fn current() -> Holder {
        let args = std::env::args_os()
            .skip(1)
            .map(|a| redact_userinfo(&a.to_string_lossy()));
        let command = std::iter::once("mind".to_string())
            .chain(args)
            .collect::<Vec<_>>()
            .join(" ");
        Holder {
            pid: std::process::id(),
            command,
            cwd: std::env::current_dir()
                .ok()
                .map(|d| d.to_string_lossy().into_owned()),
            started: now_secs(),
        }
    }

    /// The wait-step label naming this holder (PRG-4).
    fn label(&self, now: u64) -> String {
        format!(
            "{WAITING}: {} (pid {}, running {})",
            self.command,
            self.pid,
            elapsed(now.saturating_sub(self.started))
        )
    }
}

/// `12s`, `3m04s`, `1h02m`.
fn elapsed(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m{:02}s", s / 60, s % 60),
        s => format!("{}h{:02}m", s / 3600, (s % 3600) / 60),
    }
}

/// Cut the userinfo (`user:token@`) out of a URL-shaped argument, so a
/// credential typed on the command line is never written to the holder record.
fn redact_userinfo(arg: &str) -> String {
    let Some(scheme_end) = arg.find("://") else {
        return arg.to_string();
    };
    let rest_start = scheme_end + 3;
    let rest = &arg[rest_start..];
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    match rest[..authority_end].rfind('@') {
        Some(at) => format!("{}{}", &arg[..rest_start], &rest[at + 1..]),
        None => arg.to_string(),
    }
}

/// Write this process's holder record into each writable home it locked
/// (STO-84). Best effort: a record that cannot be written only costs a waiter
/// its context, never the command.
fn write_holders(paths: Vec<std::path::PathBuf>) -> Vec<std::path::PathBuf> {
    let Ok(bytes) = serde_json::to_vec(&Holder::current()) else {
        return Vec::new();
    };
    paths
        .into_iter()
        .filter(|path| write_replace(path, &bytes).is_ok())
        .collect()
}

/// The holder-record paths of the homes whose lock file opened for writing.
fn holder_paths(files: &[OneLock]) -> Vec<std::path::PathBuf> {
    files
        .iter()
        .filter(|l| l.writable)
        .map(OneLock::holder_path)
        .collect()
}

/// Replace `path` through a temp file and a rename, so a waiter never reads a
/// half-written record. No fsync: the record is meaningless after a crash.
fn write_replace(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_file_name(format!(".lock.holder.tmp.{}", std::process::id()));
    std::fs::write(&tmp, bytes)
        .and_then(|()| std::fs::rename(&tmp, path))
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
}

/// The live holder recorded beside `lock_path`, if any (STO-85). A record
/// that will not parse, or whose process is gone (a holder that crashed before
/// removing it), names no one.
pub fn read_holder(lock_path: &std::path::Path) -> Option<Holder> {
    let bytes = std::fs::read(holder_path(lock_path)).ok()?;
    let holder: Holder = serde_json::from_slice(&bytes).ok()?;
    process_alive(holder.pid).then_some(holder)
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 sends nothing; it only checks the pid exists.
    let rc = unsafe { libc::kill(pid, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn process_alive(_pid: u32) -> bool {
    true
}

const WAITING: &str = "waiting for another mind command to finish";

/// How often a waiting command retries the lock and re-reads the holder.
const POLL: std::time::Duration = std::time::Duration::from_millis(100);

/// The wait-step label for a busy lock: the recorded holder when there is a
/// live one (STO-85), else the bare label (a shared holder records nothing).
fn wait_label(lock_path: &std::path::Path) -> String {
    match read_holder(lock_path) {
        Some(h) => h.label(now_secs()),
        None => WAITING.to_string(),
    }
}

/// Whether a non-blocking lock attempt failed because another holder has the
/// lock. Any other failure is not contention: the blocking call that follows
/// reports it instead of the wait polling forever.
fn contended<T>(r: std::io::Result<T>) -> bool {
    matches!(r, Err(e) if e.kind() == std::io::ErrorKind::WouldBlock)
}

/// Wait until `busy` reports the lock free, showing a progress step labeled
/// with the current holder (PRG-4, STO-85) and refreshing it as the holder or
/// its running time changes. Returns at once when the lock is already free.
// spec: PRG-4 STO-85
fn wait_while_busy(lock_path: &std::path::Path, mut busy: impl FnMut() -> bool) {
    if !busy() {
        return;
    }
    let mut label = wait_label(lock_path);
    let step = crate::progress::step(&label);
    while busy() {
        std::thread::sleep(POLL);
        let next = wait_label(lock_path);
        if next != label {
            step.set(&next);
            label = next;
        }
    }
}

/// Set to `1` on every hook `mind` runs (HOOK-144).
pub const IN_HOOK_ENV: &str = "MIND_IN_HOOK";

/// Refuse to wait on a busy lock from inside a hook (HOOK-144): the holder is
/// the `mind` that ran the hook, which waits for the hook in turn.
// spec: HOOK-144
fn refuse_nested(busy: bool) -> Result<()> {
    if busy && std::env::var_os(IN_HOOK_ENV).is_some_and(|v| v == "1") {
        return Err(MindError::NestedInHook);
    }
    Ok(())
}

impl MindLock {
    /// Acquire the lock exclusively (mutating commands). Blocks until available.
    // spec: STO-40 STO-41 STO-42
    pub fn write(&mut self) -> Result<WriteGuard<'_>> {
        let busy = self
            .files
            .iter_mut()
            .any(|l| contended(l.inner.try_write()));
        refuse_nested(busy)?;
        let records = holder_paths(&self.files);
        let mut guards = Vec::new();
        // One home at a time in the fixed order (HOME-9), holding each lock
        // before waiting on the next, exactly as a plain blocking `write`
        // would. The probe releases at once; a holder that slips in before
        // the blocking call only means that wait shows no fresh label.
        for l in self.files.iter_mut() {
            let path = l.path.clone();
            wait_while_busy(&path, || contended(l.inner.try_write()));
            guards.push(l.inner.write().map_err(|e| MindError::io(&path, e))?);
        }
        Ok(WriteGuard {
            holders: write_holders(records),
            _guards: guards,
        })
    }

    /// Acquire the lock shared (read-only commands). Blocks until available.
    // spec: STO-40 STO-41 STO-42
    pub fn read(&self) -> Result<ReadGuard<'_>> {
        let busy = self.files.iter().any(|l| contended(l.inner.try_read()));
        refuse_nested(busy)?;
        let mut guards = Vec::new();
        for l in &self.files {
            wait_while_busy(&l.path, || contended(l.inner.try_read()));
            guards.push(l.inner.read().map_err(|e| MindError::io(&l.path, e))?);
        }
        Ok(ReadGuard { _guards: guards })
    }

    /// Try to acquire a shared lock without blocking. Returns `None` if the
    /// lock is already held exclusively by another process (e.g. during a TUI
    /// poll tick while a mutation is in progress). The TUI uses this for its
    /// ~1s refresh poll so it never freezes behind a writer (TUI-15, TUI-25).
    // spec: TUI-25
    pub fn try_read(&self) -> Option<ReadGuard<'_>> {
        let mut guards = Vec::new();
        for l in &self.files {
            guards.push(l.inner.try_read().ok()?);
        }
        Some(ReadGuard { _guards: guards })
    }

    /// Try to acquire the lock exclusively without blocking. Returns `None`
    /// while any other process holds it, so an unattended auto-update run
    /// steps aside instead of queueing behind the user (AUP-18).
    // spec: AUP-18
    pub fn try_write(&mut self) -> Option<WriteGuard<'_>> {
        let records = holder_paths(&self.files);
        let mut guards = Vec::new();
        for l in self.files.iter_mut() {
            guards.push(l.inner.try_write().ok()?);
        }
        Some(WriteGuard {
            holders: write_holders(records),
            _guards: guards,
        })
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
    fn exclusive_holder_records_itself_until_released() {
        // spec: STO-84
        let (paths, base) = temp_paths("holder");
        let lock_path = paths.lock_file();
        let mut lock = open(&paths).expect("open lock");
        let guard = lock.write().expect("exclusive acquire");
        let holder = read_holder(&lock_path).expect("a live holder is recorded");
        assert_eq!(holder.pid, std::process::id());
        assert!(holder.command.starts_with("mind"), "{holder:?}");
        assert!(holder.started > 0);
        drop(guard);
        assert!(
            !holder_path(&lock_path).exists(),
            "the record is removed on release"
        );
        cleanup(&base);
    }

    #[test]
    fn try_write_records_the_holder_and_shared_holders_record_nothing() {
        // spec: STO-84
        let (paths, base) = temp_paths("holder-try");
        let lock_path = paths.lock_file();
        {
            let lock = open(&paths).expect("open lock");
            let _shared = lock.read().expect("shared acquire");
            assert!(
                !holder_path(&lock_path).exists(),
                "a reader records nothing"
            );
        }
        let mut lock = open(&paths).expect("open lock");
        let guard = lock.try_write().expect("uncontended try_write");
        assert!(read_holder(&lock_path).is_some());
        drop(guard);
        assert!(!holder_path(&lock_path).exists());
        cleanup(&base);
    }

    #[test]
    fn a_stale_or_garbled_record_names_no_holder() {
        // spec: STO-85
        let (paths, base) = temp_paths("holder-stale");
        mkdir_p(&paths.mind_home).unwrap();
        let lock_path = paths.lock_file();
        let record = holder_path(&lock_path);
        std::fs::write(&record, b"not json").unwrap();
        assert_eq!(read_holder(&lock_path), None);
        assert_eq!(wait_label(&lock_path), WAITING);
        // A pid past any real pid_max: the holder crashed and left its record.
        let dead = Holder {
            pid: i32::MAX as u32,
            command: "mind upgrade".into(),
            cwd: None,
            started: 0,
        };
        std::fs::write(&record, serde_json::to_vec(&dead).unwrap()).unwrap();
        assert_eq!(read_holder(&lock_path), None);
        assert_eq!(wait_label(&lock_path), WAITING);
        cleanup(&base);
    }

    #[test]
    fn a_record_from_another_version_still_parses() {
        // spec: STO-86
        let (paths, base) = temp_paths("holder-compat");
        mkdir_p(&paths.mind_home).unwrap();
        let lock_path = paths.lock_file();
        let pid = std::process::id();
        std::fs::write(
            holder_path(&lock_path),
            format!(r#"{{"pid":{pid},"command":"mind sync","host":"x","extra":[1]}}"#),
        )
        .unwrap();
        let h = read_holder(&lock_path).expect("unknown fields ignored, started defaulted");
        assert_eq!(h.command, "mind sync");
        assert_eq!(h.started, 0);
        cleanup(&base);
    }

    #[test]
    fn a_lock_held_without_a_record_keeps_the_bare_label() {
        // spec: STO-86 -- an older `mind` holds the lock but writes no record.
        let (paths, base) = temp_paths("holder-old");
        mkdir_p(&paths.mind_home).unwrap();
        let lock_path = paths.lock_file();
        let mut old = RwLock::new(
            OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&lock_path)
                .unwrap(),
        );
        let _held = old.write().unwrap();
        assert!(!holder_path(&lock_path).exists());
        assert_eq!(wait_label(&lock_path), WAITING);
        assert_eq!(
            std::fs::metadata(&lock_path).unwrap().len(),
            0,
            "the lock file itself stays empty"
        );
        cleanup(&base);
    }

    #[test]
    fn wait_label_names_a_live_holder() {
        // spec: STO-85 PRG-4
        let h = Holder {
            pid: 42,
            command: "mind upgrade --yes".into(),
            cwd: Some("/w".into()),
            started: 1000,
        };
        assert_eq!(
            h.label(1075),
            "waiting for another mind command to finish: mind upgrade --yes (pid 42, running 1m15s)"
        );
        assert_eq!(elapsed(9), "9s");
        assert_eq!(elapsed(3600 + 120 + 5), "1h02m");
    }

    #[test]
    fn holder_command_drops_url_credentials() {
        // spec: STO-84
        assert_eq!(
            redact_userinfo("https://user:tok@github.com/o/r"),
            "https://github.com/o/r"
        );
        assert_eq!(
            redact_userinfo("ssh://git@host/o/r#a@b"),
            "ssh://host/o/r#a@b"
        );
        assert_eq!(redact_userinfo("o/r@v1"), "o/r@v1");
        assert_eq!(redact_userinfo("--yes"), "--yes");
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
