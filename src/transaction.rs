//! Durable transaction journals for installs and manifest publication.
//!
//! A journal records each filesystem operation before it is attempted, then a
//! commit decision once the manifests that describe the result are known.
//! Recovery after an interruption reads the journal back: before the commit
//! decision it undoes every recorded operation in reverse (LIFE-59); after it,
//! it finishes publishing the recorded manifests and empties the transaction's
//! trash (LIFE-60).
//!
//! Two journal sites exist:
//!
//! - `<mind home>/.tx-journal.json` (this module): one per command, in the
//!   home of its first change (HOME-1). Every install and uninstall step joins
//!   it ([`scope`]); a manifest save commits it ([`commit_manifests`]), across
//!   every home at once.
//! - `<home>/.manifest-journal.json` (`manifest.rs`, STO-87): a manifest save
//!   made outside a command transaction publishes each home on its own.
//!
//! Replaced store copies and link targets are not deleted while a transaction
//! is open: they move to a trash directory (`<home>/.tmp/tx/<id>/<n>`) that is
//! emptied after the commit, so rollback can always move them back.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

use crate::error::{MindError, Result};
use crate::paths::{Paths, normalize_home};

const VERSION: u32 = 1;
// Journal contents originate in writable mind homes. Bound both reads and
// writes so an oversized or malicious journal cannot exhaust memory and we
// never commit a journal that this version cannot subsequently recover.
const MAX_JOURNAL_BYTES: usize = 16 * 1024 * 1024;
static TEMP_ID: AtomicU64 = AtomicU64::new(0);

/// The command transaction's journal file name, in the home of its first change.
pub(crate) const TX_JOURNAL: &str = ".tx-journal.json";

/// An intent is recorded before its filesystem operation is attempted. Undo is
/// idempotent, so recovery need not know whether the operation ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) enum Intent {
    /// A rename. Undo moves it back. `identity` is the moved entry's
    /// (device, inode) where the platform has one, so undo never moves an
    /// entry this transaction did not put there (LIFE-64).
    Move {
        from: PathBuf,
        to: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        identity: Option<FileId>,
    },
    /// A symlink at `path` to `target` was created. Undo removes it.
    CreateLink { path: PathBuf, target: PathBuf },
    /// mind's own symlink at `path` (to `target`) was removed. Undo recreates it.
    RemoveLink { path: PathBuf, target: PathBuf },
    /// The intended manifest bytes are retained in the journal so replay can
    /// verify a destination even if a completed rename consumed the stage.
    /// `prior` is the destination before this transaction: publishing never
    /// overwrites a file that changed since (LIFE-64).
    Manifest {
        staged: PathBuf,
        destination: PathBuf,
        contents: Vec<u8>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prior: Option<Prior>,
    },
    /// A hook started. Its effects are not undone (LIFE-62).
    Hook { label: String },
    /// The most recent [`Intent::Hook`] finished.
    HookDone,
}

/// A filesystem entry's (device, inode). A rename keeps it.
pub(crate) type FileId = (u64, u64);

#[cfg(unix)]
fn file_id(meta: &fs::Metadata) -> Option<FileId> {
    use std::os::unix::fs::MetadataExt;
    Some((meta.dev(), meta.ino()))
}

#[cfg(not(unix))]
fn file_id(_meta: &fs::Metadata) -> Option<FileId> {
    None
}

/// The identity of the entry at `path` (not following a symlink), if any.
fn identity_of(path: &Path) -> Option<FileId> {
    fs::symlink_metadata(path).ok().as_ref().and_then(file_id)
}

/// A file's state before a transaction replaced it (LIFE-64).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Prior {
    Absent,
    /// [`crate::hash::fingerprint_bytes`] of its contents.
    Contents(String),
}

impl Prior {
    pub(crate) fn of(path: &Path) -> io::Result<Self> {
        match fs::read(path) {
            Ok(bytes) => Ok(Self::Contents(crate::hash::fingerprint_bytes(&bytes))),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::Absent),
            Err(e) => Err(e),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Decision {
    Preparing,
    Commit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Journal {
    version: u32,
    pub(crate) decision: Decision,
    pub(crate) intents: Vec<Intent>,
    /// The journal's file; `None` for a volatile (in-memory) journal.
    #[serde(skip)]
    file: Option<PathBuf>,
}

impl Journal {
    fn new(file: Option<PathBuf>) -> Self {
        Self {
            version: VERSION,
            decision: Decision::Preparing,
            intents: Vec::new(),
            file,
        }
    }

    /// An in-memory journal: the same undo bookkeeping with nothing on disk.
    pub(crate) fn volatile() -> Self {
        Self::new(None)
    }

    /// Persist a new intent before the corresponding action. Callers must not
    /// begin applying operations until this method succeeds.
    pub(crate) fn record(&mut self, intent: Intent) -> io::Result<()> {
        if self.decision != Decision::Preparing {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "transaction already committed",
            ));
        }
        let mut next = self.clone();
        next.intents.push(intent);
        next.save()?;
        *self = next;
        Ok(())
    }

    /// Drop the last intent once it is undone (or never ran).
    fn pop(&mut self) -> io::Result<()> {
        let mut next = self.clone();
        next.intents.pop();
        next.save()?;
        *self = next;
        Ok(())
    }

    /// Undo every intent past `start`, newest first, dropping each from the
    /// journal once undone. An interrupted undo may be retried: each step is
    /// idempotent and the journal only shrinks after a step completes.
    fn undo_to(&mut self, start: usize) -> io::Result<()> {
        if self.decision != Decision::Preparing {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a committed transaction cannot be undone",
            ));
        }
        self.check_undo(start)?;
        while self.intents.len() > start {
            let last = self.intents.last().expect("non-empty").clone();
            undo(&last)?;
            crash_point();
            self.pop()?;
            crash_point();
        }
        Ok(())
    }

    /// Publish the exact intended manifest bytes after a commit decision. The
    /// caller must validate destinations and hold the exclusive state lock.
    /// Bytes are retained in the journal, so retry works after a completed
    /// rename consumed a staging file.
    pub(crate) fn finish_prevalidated_manifests(&self) -> io::Result<()> {
        if self.decision != Decision::Commit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "transaction is not committed",
            ));
        }
        // spec: LIFE-64 -- check every destination before publishing any, so
        // a manifest changed by another mind stops the commit whole.
        for intent in &self.intents {
            if let Intent::Manifest {
                destination,
                contents,
                prior: Some(prior),
                ..
            } = intent
            {
                check_unchanged(destination, contents, prior)?;
            }
        }
        for intent in &self.intents {
            if let Intent::Manifest {
                destination,
                contents,
                ..
            } = intent
            {
                publish(destination, contents)?;
                crash_point();
            }
        }
        Ok(())
    }

    /// Check that undoing every intent past `start`, newest first, finds each
    /// path in the state this transaction left (LIFE-64). The steps are
    /// replayed on a view of the files, since each undo changes what the
    /// next one finds; nothing on disk is touched.
    fn check_undo(&self, start: usize) -> io::Result<()> {
        use std::collections::HashMap;
        // `None`: absent. `Some(id)`: present, with its identity if known.
        let mut view: HashMap<PathBuf, Option<Option<FileId>>> = HashMap::new();
        let state = |view: &HashMap<PathBuf, Option<Option<FileId>>>, path: &Path| {
            if let Some(state) = view.get(path) {
                return Ok((*state, true));
            }
            match fs::symlink_metadata(path) {
                Ok(meta) => Ok((Some(file_id(&meta)), false)),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok((None, false)),
                Err(e) => Err(e),
            }
        };
        for intent in self.intents[start..].iter().rev() {
            match intent {
                Intent::Move { from, to, identity } => {
                    match (state(&view, from)?.0, state(&view, to)?.0) {
                        (Some(_), None) => {}
                        (None, Some(found)) => {
                            if let (Some(want), Some(found)) = (identity, found)
                                && *want != found
                            {
                                return Err(ambiguous(format!(
                                    "'{}' is no longer what this transaction moved there (another mind may have changed it)",
                                    to.display()
                                )));
                            }
                            view.insert(from.clone(), Some(found));
                            view.insert(to.clone(), None);
                        }
                        _ => {
                            return Err(ambiguous(format!(
                                "cannot tell whether '{}' was moved to '{}'",
                                from.display(),
                                to.display()
                            )));
                        }
                    }
                }
                Intent::CreateLink { path, target } => {
                    let (found, simulated) = state(&view, path)?;
                    if found.is_some()
                        && !simulated
                        && fs::read_link(path).ok().as_ref() != Some(target)
                    {
                        return Err(ambiguous(format!(
                            "'{}' is no longer the link this transaction created",
                            path.display()
                        )));
                    }
                    view.insert(path.clone(), None);
                }
                Intent::RemoveLink { path, target } => {
                    let (found, simulated) = state(&view, path)?;
                    if found.is_some()
                        && !simulated
                        && fs::read_link(path).ok().as_ref() != Some(target)
                    {
                        return Err(ambiguous(format!(
                            "'{}' is occupied; cannot restore its link to '{}'",
                            path.display(),
                            target.display()
                        )));
                    }
                    view.insert(path.clone(), Some(None));
                }
                Intent::Manifest { .. } | Intent::Hook { .. } | Intent::HookDone => {}
            }
        }
        Ok(())
    }

    /// Once recorded, a commit decision is irreversible and recovery must
    /// finish manifest publication instead of rolling back.
    pub(crate) fn commit(&mut self) -> io::Result<()> {
        if self.decision != Decision::Preparing {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "transaction already committed",
            ));
        }
        let mut next = self.clone();
        next.decision = Decision::Commit;
        next.save()?;
        *self = next;
        Ok(())
    }

    pub(crate) fn load(file: &Path) -> io::Result<Self> {
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let input = options.open(file)?;
        if !input.metadata()?.file_type().is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "transaction journal must be a regular file",
            ));
        }
        let mut bytes = Vec::new();
        input
            .take((MAX_JOURNAL_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_JOURNAL_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "transaction journal exceeds size limit",
            ));
        }
        let mut journal: Self = serde_json::from_slice(&bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        if journal.version != VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported transaction journal version",
            ));
        }
        journal.file = Some(file.to_path_buf());
        Ok(journal)
    }

    /// Create a new journal exclusively; never replace an unfinished one.
    pub(crate) fn begin(file: &Path) -> io::Result<Self> {
        let journal = Self::new(Some(file.to_path_buf()));
        let bytes = journal.serialized()?;
        let temp = temp_path(file)?;
        let result = (|| {
            let mut output = open_private_new(&temp)?;
            output.write_all(&bytes)?;
            output.sync_all()?;
            // Unlike rename, hard_link refuses to replace an existing journal.
            // The published file therefore contains either a complete journal
            // or the previous transaction's journal, never partial bytes.
            fs::hard_link(&temp, file)?;
            sync_parent(file)
        })();
        let _ = fs::remove_file(&temp);
        result?;
        Ok(journal)
    }

    /// Delete the journal file: the transaction is settled.
    pub(crate) fn remove(&self) -> io::Result<()> {
        let Some(file) = &self.file else {
            return Ok(());
        };
        match fs::remove_file(file) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        sync_parent(file)
    }

    fn save(&self) -> io::Result<()> {
        let bytes = self.serialized()?;
        let Some(file) = &self.file else {
            return Ok(());
        };
        let temp = temp_path(file)?;
        let result = (|| {
            let mut output = open_private_new(&temp)?;
            output.write_all(&bytes)?;
            output.sync_all()?;
            fs::rename(&temp, file)?;
            sync_parent(file)
        })();
        if result.is_err() {
            let _ = fs::remove_file(temp);
        }
        result
    }

    fn serialized(&self) -> io::Result<Vec<u8>> {
        struct Bounded(Vec<u8>);
        impl Write for Bounded {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if bytes.len() > MAX_JOURNAL_BYTES.saturating_sub(self.0.len()) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "transaction journal exceeds size limit",
                    ));
                }
                self.0.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut output = Bounded(Vec::new());
        serde_json::to_writer(&mut output, self).map_err(io::Error::other)?;
        Ok(output.0)
    }

    /// Hooks that started and did not finish, oldest first.
    fn interrupted_hooks(&self) -> Vec<String> {
        let mut open: Vec<String> = Vec::new();
        for intent in &self.intents {
            match intent {
                Intent::Hook { label } => open.push(label.clone()),
                Intent::HookDone => {
                    open.pop();
                }
                _ => {}
            }
        }
        open
    }
}

/// Reverse one recorded operation. Each case is idempotent: a step that never
/// ran, or was already undone, is left alone. A state matching neither the
/// before nor the after of the step fails closed.
fn undo(intent: &Intent) -> io::Result<()> {
    match intent {
        Intent::Move { from, to, identity } => match (path_present(from)?, path_present(to)?) {
            (true, false) => Ok(()), // Not started, or already undone.
            (false, true) => {
                if let (Some(want), Some(found)) = (identity, identity_of(to))
                    && *want != found
                {
                    return Err(ambiguous(format!(
                        "'{}' is no longer what this transaction moved there (another mind may have changed it)",
                        to.display()
                    )));
                }
                fs::rename(to, from)?;
                sync_parent(from)?;
                if from.parent() != to.parent() {
                    sync_parent(to)?;
                }
                Ok(())
            }
            _ => Err(ambiguous(format!(
                "cannot tell whether '{}' was moved to '{}'",
                from.display(),
                to.display()
            ))),
        },
        Intent::CreateLink { path, target } => match fs::symlink_metadata(path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
            Ok(meta) if meta.file_type().is_symlink() && fs::read_link(path)? == *target => {
                fs::remove_file(path)?;
                sync_parent(path)
            }
            Ok(_) => Err(ambiguous(format!(
                "'{}' is no longer the link this transaction created",
                path.display()
            ))),
        },
        Intent::RemoveLink { path, target } => match fs::symlink_metadata(path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                make_symlink(target, path)?;
                sync_parent(path)
            }
            Err(e) => Err(e),
            Ok(meta) if meta.file_type().is_symlink() && fs::read_link(path)? == *target => Ok(()),
            Ok(_) => Err(ambiguous(format!(
                "'{}' is occupied; cannot restore its link to '{}'",
                path.display(),
                target.display()
            ))),
        },
        Intent::Manifest { .. } | Intent::Hook { .. } | Intent::HookDone => Ok(()),
    }
}

fn ambiguous(msg: String) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("{msg}; retained for diagnosis"),
    )
}

/// Fail closed unless `destination` holds `contents` already or is still as
/// `prior` records (LIFE-64).
fn check_unchanged(destination: &Path, contents: &[u8], prior: &Prior) -> io::Result<()> {
    let now = match fs::read(destination) {
        Ok(existing) if existing == *contents => return Ok(()),
        Ok(existing) => Prior::Contents(crate::hash::fingerprint_bytes(&existing)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Prior::Absent,
        Err(e) => return Err(e),
    };
    if now == *prior {
        return Ok(());
    }
    Err(ambiguous(format!(
        "'{}' changed after this transaction began (another mind may have written it); publishing would overwrite that change",
        destination.display()
    )))
}

/// Write `contents` to `destination` atomically, unless it already holds them.
fn publish(destination: &Path, contents: &[u8]) -> io::Result<()> {
    match fs::read(destination) {
        Ok(existing) if existing == *contents => {
            // A previous process may have renamed this file and crashed
            // before its parent directory was synced.
            return sync_parent(destination);
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let temp = temp_path(destination)?;
    let result = (|| {
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        output.write_all(contents)?;
        output.sync_all()?;
        fs::rename(&temp, destination)?;
        sync_parent(destination)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// A trash entry is `<home>/.tmp/tx/<id>/<n>`.
fn is_trash(path: &Path) -> bool {
    path.parent()
        .and_then(Path::parent)
        .is_some_and(|p| p.ends_with(Path::new(".tmp").join("tx")))
}

/// Delete what a settled transaction moved to trash, then the emptied
/// per-transaction trash directories.
fn empty_trash(intents: &[Intent]) -> io::Result<()> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for intent in intents {
        if let Intent::Move { to, .. } = intent
            && is_trash(to)
        {
            remove_any(to)?;
            if let Some(dir) = to.parent()
                && !dirs.iter().any(|d| d == dir)
            {
                dirs.push(dir.to_path_buf());
            }
        }
    }
    for dir in dirs {
        let _ = fs::remove_dir(dir);
    }
    Ok(())
}

fn remove_any(path: &Path) -> io::Result<()> {
    let result = match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
    };
    match result {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

fn path_present(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn temp_path(file: &Path) -> io::Result<PathBuf> {
    let parent = file
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "journal has no parent"))?;
    let mut name = file
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "journal has no name"))?
        .to_os_string();
    name.push(format!(
        ".tmp.{}.{}",
        std::process::id(),
        TEMP_ID.fetch_add(1, Ordering::Relaxed)
    ));
    Ok(parent.join(name))
}

fn open_private_new(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

#[cfg(unix)]
fn make_symlink(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn make_symlink(target: &Path, link: &Path) -> io::Result<()> {
    if target.is_dir() {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    }
}

/// Sync the directory entry after journal, manifest, or stash renames.
pub(crate) fn sync_parent(path: &Path) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
    File::open(parent)?.sync_all()
}

/// Check lexical containment only. Recovery additionally checks each path's
/// operation type and symlink ancestors ([`ancestors_confined`]).
pub(crate) fn lexically_contained(path: &Path, root: &Path) -> bool {
    path.is_absolute()
        && root.is_absolute()
        && path
            .components()
            .all(|c| !matches!(c, Component::ParentDir | Component::CurDir))
        && root
            .components()
            .all(|c| !matches!(c, Component::ParentDir | Component::CurDir))
        && path != root
        && path.starts_with(root)
}

/// True when the nearest existing ancestor of `path`, with every symlink
/// resolved, is still under `root` (also resolved): no ancestor redirects the
/// path elsewhere. A root that does not exist cannot redirect anything.
fn ancestors_confined(path: &Path, root: &Path) -> bool {
    let Ok(root) = root.canonicalize() else {
        return true;
    };
    path.ancestors()
        .skip(1)
        .find_map(|a| a.canonicalize().ok())
        .is_some_and(|existing| existing.starts_with(&root))
}

// ---------------------------------------------------------------------------
// Crash injection (LIFE-63)
// ---------------------------------------------------------------------------

/// Test-only fault injection (LIFE-63): with `MIND_TEST_CRASH_AT=<n>`, the
/// process exits abruptly (no destructors, no cleanup) at the n-th journal
/// transition, as if it were killed there. Debug builds only.
pub(crate) const CRASH_ENV: &str = "MIND_TEST_CRASH_AT";
/// The exit status of an injected crash.
pub(crate) const CRASH_STATUS: i32 = 86;

#[cfg(debug_assertions)]
fn crash_point() {
    static COUNT: AtomicU64 = AtomicU64::new(0);
    static AT: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    let Some(at) = *AT.get_or_init(|| std::env::var(CRASH_ENV).ok()?.parse().ok()) else {
        return;
    };
    if COUNT.fetch_add(1, Ordering::SeqCst) + 1 == at {
        let _ = std::io::stdout().flush();
        // SAFETY: `_exit` ends the process at once; that is the point.
        unsafe { libc::_exit(CRASH_STATUS) };
    }
}

#[cfg(not(debug_assertions))]
fn crash_point() {}

// ---------------------------------------------------------------------------
// The active transaction
// ---------------------------------------------------------------------------

struct Active {
    /// The configured homes (default first) a durable journal may go in, or
    /// `None` for a volatile transaction.
    durable: Option<Vec<PathBuf>>,
    /// The open journal; `None` until the first operation is recorded.
    journal: Option<Journal>,
    id: String,
    seq: u64,
    /// A rollback or a commit failed part-way: the journal on disk is the
    /// record now, and recovery must settle it before anything else runs.
    poisoned: bool,
}

impl Active {
    fn new(durable: Option<Vec<PathBuf>>) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        Self {
            durable,
            journal: None,
            id: format!("{}-{nanos}", std::process::id()),
            seq: 0,
            poisoned: false,
        }
    }

    fn journal_path(&self) -> PathBuf {
        if let Some(file) = self.journal.as_ref().and_then(|j| j.file.clone()) {
            return file;
        }
        self.durable
            .as_ref()
            .and_then(|homes| homes.first())
            .map(|home| home.join(TX_JOURNAL))
            .unwrap_or_default()
    }

    /// The home the journal goes in: the first configured home that holds
    /// `hint` (HOME-1: work confined to one home writes nothing in another),
    /// else the default home.
    fn home_for(homes: &[PathBuf], hint: Option<&Path>) -> PathBuf {
        hint.and_then(|hint| {
            let hint = normalize_home(hint);
            homes.iter().find(|home| hint.starts_with(home)).cloned()
        })
        .or_else(|| homes.first().cloned())
        .unwrap_or_default()
    }

    fn io(&self, e: io::Error) -> MindError {
        MindError::io(self.journal_path(), e)
    }

    fn journal(&mut self, hint: Option<&Path>) -> Result<&mut Journal> {
        if self.journal.is_none() {
            let journal = match &self.durable {
                Some(homes) => {
                    let file = Self::home_for(homes, hint).join(TX_JOURNAL);
                    Journal::begin(&file).map_err(|e| MindError::io(&file, e))?
                }
                None => Journal::volatile(),
            };
            self.journal = Some(journal);
        }
        Ok(self.journal.as_mut().expect("just set"))
    }

    fn len(&self) -> usize {
        self.journal.as_ref().map_or(0, |j| j.intents.len())
    }

    fn record(&mut self, intent: Intent, hint: Option<&Path>) -> Result<()> {
        let hint = hint.or(match &intent {
            Intent::Move { to, .. } => Some(to.as_path()),
            Intent::CreateLink { target, .. } | Intent::RemoveLink { target, .. } => {
                Some(target.as_path())
            }
            Intent::Manifest { destination, .. } => Some(destination.as_path()),
            Intent::Hook { .. } | Intent::HookDone => None,
        });
        self.journal(hint)?;
        let file = self.journal_path();
        self.journal
            .as_mut()
            .expect("opened above")
            .record(intent)
            .map_err(|e| MindError::io(&file, e))?;
        crash_point();
        Ok(())
    }

    /// Forget the last intent: its operation failed without changing anything.
    fn unrecord(&mut self) {
        let result = match self.journal.as_mut() {
            Some(j) => j.pop(),
            None => Ok(()),
        };
        if result.is_err() {
            self.poisoned = true;
        }
    }

    fn trash_path(&mut self, home: &Path) -> PathBuf {
        self.seq += 1;
        normalize_home(home)
            .join(".tmp")
            .join("tx")
            .join(&self.id)
            .join(self.seq.to_string())
    }

    fn rollback_to(&mut self, start: usize) -> Result<()> {
        let file = self.journal_path();
        if let Some(journal) = self.journal.as_mut() {
            journal
                .undo_to(start)
                .map_err(|e| MindError::io(&file, e))?;
        }
        Ok(())
    }

    /// Settle with no manifest to publish: commit what ran, empty the trash.
    fn commit_without_manifests(&mut self) -> Result<()> {
        let Some(mut journal) = self.journal.take() else {
            return Ok(());
        };
        if journal.intents.is_empty() {
            return journal.remove().map_err(|e| self.io(e));
        }
        journal.commit().map_err(|e| self.io(e))?;
        crash_point();
        empty_trash(&journal.intents).map_err(|e| self.io(e))?;
        crash_point();
        journal.remove().map_err(|e| self.io(e))
    }
}

thread_local! {
    // The transaction belongs to the thread running the command: installs run
    // on that thread, and in-process tests run commands on parallel threads
    // that must not join each other's transactions. One small allocation per
    // thread is leaked to hand out a `'static` guard.
    static ACTIVE: &'static Mutex<Option<Active>> = Box::leak(Box::new(Mutex::new(None)));
}

fn active() -> MutexGuard<'static, Option<Active>> {
    ACTIVE
        .with(|slot| *slot)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn poisoned_error(path: PathBuf) -> MindError {
    MindError::TransactionJournal {
        path,
        reason: "an earlier step could not be rolled back; run `mind recall` to recover".into(),
    }
}

/// Run `op` inside the active transaction (a [`Scope`] guarantees one).
fn with_active<R>(op: impl FnOnce(&mut Active) -> Result<R>) -> Result<R> {
    let mut guard = active();
    let tx = guard
        .as_mut()
        .expect("a transaction operation runs inside a transaction scope");
    if tx.poisoned {
        return Err(poisoned_error(tx.journal_path()));
    }
    op(tx)
}

/// The configured homes, normalized, the default first.
fn homes(paths: &Paths) -> Vec<PathBuf> {
    let mut homes: Vec<PathBuf> = Vec::new();
    for home in paths.all_homes() {
        let home = normalize_home(&home);
        if !homes.contains(&home) {
            homes.push(home);
        }
    }
    homes
}

/// Where a command transaction's journal may be: one per configured home.
/// A transaction keeps its journal in the home of its first change, so work
/// confined to an extra home writes nothing in the default one (HOME-1).
pub(crate) fn journal_files(paths: &Paths) -> Vec<PathBuf> {
    homes(paths).iter().map(|h| h.join(TX_JOURNAL)).collect()
}

/// A command's transaction (LIFE-59): opened while the exclusive state lock is
/// held, after [`recover`]. [`Guard::finish`] commits what is still pending;
/// dropping it unfinished (an error, a panic) rolls back everything recorded
/// since the last manifest save.
#[must_use]
pub(crate) struct Guard {
    owned: bool,
}

/// Open the command transaction. A transaction already open (a TUI action
/// inside a session) is joined instead.
pub(crate) fn begin(paths: &Paths) -> Guard {
    let mut guard = active();
    if guard.is_some() {
        return Guard { owned: false };
    }
    *guard = Some(Active::new(Some(homes(paths))));
    Guard { owned: true }
}

impl Guard {
    /// Settle the transaction by the command's outcome: commit on success,
    /// roll back on failure. Returns the settlement error, if any.
    pub(crate) fn settle<T>(self, outcome: &Result<T>) -> Result<()> {
        if outcome.is_ok() {
            self.finish()
        } else {
            drop(self);
            Ok(())
        }
    }

    /// Commit anything still pending and close the transaction.
    pub(crate) fn finish(mut self) -> Result<()> {
        if !self.owned {
            return Ok(());
        }
        self.owned = false;
        let Some(mut tx) = active().take() else {
            return Ok(());
        };
        if tx.poisoned {
            return Err(poisoned_error(tx.journal_path()));
        }
        tx.commit_without_manifests()
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if !self.owned {
            return;
        }
        let Some(mut tx) = active().take() else {
            return;
        };
        if tx.poisoned {
            return;
        }
        match tx.rollback_to(0) {
            Ok(()) => {
                if let Some(journal) = tx.journal.take() {
                    let _ = journal.remove();
                }
            }
            Err(e) => crate::render::warn(format!(
                "could not roll back an unfinished change: {}; run `mind recall` to recover",
                crate::sanitize::strip_ansi(&e.to_string())
            )),
        }
    }
}

/// A unit of work inside the transaction, such as one item's install (the
/// savepoint of LIFE-4): [`Scope::done`] keeps its operations, dropping it
/// unfinished undoes them. With no command transaction open, the scope runs
/// its own volatile one, which `done` settles at once.
#[must_use]
pub(crate) struct Scope {
    start: usize,
    owned: bool,
    settled: bool,
}

pub(crate) fn scope() -> Scope {
    let mut guard = active();
    match guard.as_ref() {
        Some(tx) => Scope {
            start: tx.len(),
            owned: false,
            settled: false,
        },
        None => {
            *guard = Some(Active::new(None));
            Scope {
                start: 0,
                owned: true,
                settled: false,
            }
        }
    }
}

impl Scope {
    /// Keep this scope's operations. A volatile scope settles them now.
    pub(crate) fn done(mut self) -> Result<()> {
        self.settled = true;
        if !self.owned {
            return Ok(());
        }
        match active().take() {
            Some(mut tx) => tx.commit_without_manifests(),
            None => Ok(()),
        }
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        let mut guard = active();
        if let Some(tx) = guard.as_mut()
            && !tx.poisoned
            && tx.rollback_to(self.start).is_err()
        {
            tx.poisoned = true;
        }
        if self.owned {
            *guard = None;
        }
    }
}

/// Rename `from` to `to` (both in one filesystem), recorded first.
pub(crate) fn move_path(from: &Path, to: &Path) -> Result<()> {
    let (from, to) = (normalize_home(from), normalize_home(to));
    if let Some(parent) = to.parent() {
        crate::paths::mkdir_p(parent)?;
    }
    with_active(|tx| {
        tx.record(
            Intent::Move {
                from: from.clone(),
                to: to.clone(),
                identity: identity_of(&from),
            },
            None,
        )?;
        if let Err(e) = fs::rename(&from, &to) {
            // A failed rename changed nothing, so the intent goes too.
            tx.unrecord();
            return Err(MindError::io(&to, e));
        }
        // A failed sync after the rename did change something: the intent
        // stays, so a rollback moves it back.
        let synced = sync_parent(&from).and_then(|()| {
            if from.parent() != to.parent() {
                sync_parent(&to)?;
            }
            Ok(())
        });
        if let Err(e) = synced {
            return Err(MindError::io(&to, e));
        }
        crash_point();
        Ok(())
    })
}

/// Move `path` aside into this transaction's trash in `home` (same
/// filesystem), so a rollback can move it back and a commit deletes it.
pub(crate) fn move_aside(path: &Path, home: &Path) -> Result<()> {
    let trash = with_active(|tx| Ok(tx.trash_path(home)))?;
    move_path(path, &trash)
}

/// Create a symlink at `link` to `target`, recorded first.
pub(crate) fn create_link(target: &Path, link: &Path) -> Result<()> {
    let link = normalize_home(link);
    if let Some(parent) = link.parent() {
        crate::paths::mkdir_p(parent)?;
    }
    with_active(|tx| {
        tx.record(
            Intent::CreateLink {
                path: link.clone(),
                target: target.to_path_buf(),
            },
            None,
        )?;
        if let Err(e) = make_symlink(target, &link) {
            tx.unrecord();
            return Err(MindError::io(&link, e));
        }
        crash_point();
        Ok(())
    })
}

/// Remove mind's own symlink at `link`, recording its target first so a
/// rollback can recreate it.
pub(crate) fn remove_link(link: &Path) -> Result<()> {
    let link = normalize_home(link);
    let target = fs::read_link(&link).map_err(|e| MindError::io(&link, e))?;
    with_active(|tx| {
        tx.record(
            Intent::RemoveLink {
                path: link.clone(),
                target,
            },
            None,
        )?;
        if let Err(e) = fs::remove_file(&link) {
            tx.unrecord();
            return Err(MindError::io(&link, e));
        }
        crash_point();
        Ok(())
    })
}

/// Record that a hook is starting (LIFE-62), when a command transaction is
/// open. Its effects are outside the journal; recovery reports it instead.
/// `site` (the hook's working directory) places a journal not yet open in
/// the home holding it.
pub(crate) fn hook_started(label: &str, site: &Path) {
    let mut guard = active();
    if let Some(tx) = guard.as_mut()
        && tx.durable.is_some()
        && !tx.poisoned
        && tx
            .record(
                Intent::Hook {
                    label: label.to_string(),
                },
                Some(site),
            )
            .is_err()
    {
        tx.poisoned = true;
    }
}

/// Record that the hook [`hook_started`] announced has finished.
pub(crate) fn hook_finished() {
    let mut guard = active();
    if let Some(tx) = guard.as_mut()
        && tx.durable.is_some()
        && tx.journal.is_some()
        && !tx.poisoned
        && tx.record(Intent::HookDone, None).is_err()
    {
        tx.poisoned = true;
    }
}

/// Commit the command transaction with these manifest writes (LIFE-60):
/// record every home's intended manifest, record the commit decision, publish
/// them, and empty the trash. Returns `false` when no command transaction is
/// open, so the caller publishes on its own (STO-87).
pub(crate) fn commit_manifests(writes: &[(PathBuf, Vec<u8>)]) -> Result<bool> {
    let mut guard = active();
    let Some(tx) = guard.as_mut().filter(|tx| tx.durable.is_some()) else {
        return Ok(false);
    };
    if tx.poisoned {
        return Err(poisoned_error(tx.journal_path()));
    }
    for (destination, contents) in writes {
        let destination = normalize_home(destination);
        let prior = Prior::of(&destination).map_err(|e| MindError::io(&destination, e))?;
        tx.record(
            Intent::Manifest {
                staged: destination.clone(),
                destination,
                contents: contents.clone(),
                prior: Some(prior),
            },
            None,
        )?;
    }
    if tx.journal.is_none() {
        // Nothing recorded and no manifest changed: nothing to commit.
        return Ok(true);
    }
    let file = tx.journal_path();
    let mut journal = tx.journal.take().expect("checked above");
    // From the decision on, failure leaves the journal for recovery to finish.
    tx.poisoned = true;
    journal.commit().map_err(|e| MindError::io(&file, e))?;
    crash_point();
    journal
        .finish_prevalidated_manifests()
        .map_err(|e| MindError::io(&file, e))?;
    empty_trash(&journal.intents).map_err(|e| MindError::io(&file, e))?;
    crash_point();
    journal.remove().map_err(|e| MindError::io(&file, e))?;
    crash_point();
    tx.poisoned = false;
    Ok(true)
}

// ---------------------------------------------------------------------------
// Recovery (LIFE-61)
// ---------------------------------------------------------------------------

/// Whether any journal awaits recovery: the command transaction's, or a
/// per-home manifest journal (STO-87).
pub(crate) fn pending(paths: &Paths) -> Result<bool> {
    for file in journal_files(paths) {
        match fs::symlink_metadata(&file) {
            Ok(_) => return Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(MindError::io(&file, e)),
        }
    }
    crate::manifest::manifest_journal_pending(paths)
}

/// Refuse to go on while recovery is pending: a dry run must not write
/// state, and recovery does (STO-87, LIFE-61).
pub(crate) fn refuse_pending(paths: &Paths) -> Result<()> {
    for file in journal_files(paths) {
        match fs::symlink_metadata(&file) {
            Ok(_) => return Err(MindError::RecoveryPending { path: file }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(MindError::io(&file, e)),
        }
    }
    crate::manifest::refuse_pending_recovery(paths)
}

/// Recover every unfinished journal. The caller holds the exclusive state
/// lock. Per-home manifest journals go first, so the manifests the command
/// journal's validation reads are whole.
pub(crate) fn recover(paths: &Paths) -> Result<()> {
    crate::manifest::recover_manifest_journal(paths)?;
    for file in journal_files(paths) {
        recover_command_journal(paths, &file)?;
    }
    Ok(())
}

fn invalid(file: &Path, reason: impl ToString) -> MindError {
    MindError::TransactionJournal {
        path: file.to_path_buf(),
        reason: reason.to_string(),
    }
}

fn journal_error(file: &Path, error: io::Error) -> MindError {
    if error.kind() == io::ErrorKind::InvalidData {
        invalid(file, error)
    } else {
        MindError::io(file, error)
    }
}

fn recover_command_journal(paths: &Paths, file: &Path) -> Result<()> {
    let file = file.to_path_buf();
    match fs::symlink_metadata(&file) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(invalid(&file, "the journal must not be a symlink"));
        }
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(MindError::io(&file, e)),
    }
    let mut journal = match Journal::load(&file) {
        Ok(journal) => journal,
        Err(e) if e.kind() == io::ErrorKind::NotFound => match fs::symlink_metadata(&file) {
            Err(missing) if missing.kind() == io::ErrorKind::NotFound => return Ok(()),
            Ok(_) => return Err(invalid(&file, "the journal is a dangling symlink")),
            Err(other) => return Err(MindError::io(&file, other)),
        },
        Err(e) => return Err(journal_error(&file, e)),
    };
    let roots = Roots::of(paths);
    for intent in &journal.intents {
        roots
            .check(intent)
            .map_err(|reason| invalid(&file, reason))?;
    }
    match journal.decision {
        Decision::Commit => {
            journal
                .finish_prevalidated_manifests()
                .map_err(|e| journal_error(&file, e))?;
            empty_trash(&journal.intents).map_err(|e| journal_error(&file, e))?;
            crash_point();
        }
        Decision::Preparing => {
            let hooks = journal.interrupted_hooks();
            journal.undo_to(0).map_err(|e| journal_error(&file, e))?;
            // spec: LIFE-62 -- a hook's own effects are not undone; say so.
            let note = match hooks.as_slice() {
                [] => String::new(),
                hooks => format!(
                    "; hook(s) {} had started, and their effects were not undone, so inspect them",
                    hooks
                        .iter()
                        .map(|h| format!("'{}'", crate::sanitize::strip_ansi(h)))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            };
            crate::render::warn(format!(
                "rolled back an interrupted change to installed items{note}"
            ));
        }
    }
    journal.remove().map_err(|e| MindError::io(&file, e))
}

/// Where a recovered journal may act (LIFE-61): the configured homes' stores
/// and `.tmp` trees, the lobes, and each home's manifest.
struct Roots {
    homes: Vec<PathBuf>,
    stores: Vec<PathBuf>,
    tmps: Vec<PathBuf>,
    lobes: Vec<PathBuf>,
}

impl Roots {
    fn of(paths: &Paths) -> Self {
        let homes: Vec<PathBuf> = paths
            .all_homes()
            .iter()
            .map(|h| normalize_home(h))
            .collect();
        let stores = homes.iter().map(|h| h.join("store")).collect();
        let tmps = homes.iter().map(|h| h.join(".tmp")).collect();
        let mut lobes: Vec<PathBuf> = Vec::new();
        if let Ok(homes) = paths.agent_homes() {
            lobes.extend(homes.into_iter().map(|l| normalize_home(&l.path)));
        }
        if let Ok(registry) = crate::source::Registry::load(paths) {
            for source in &registry.sources {
                if let Ok(Some(lobe)) = paths.source_lobe(&source.name) {
                    lobes.push(normalize_home(&lobe));
                }
            }
        }
        if let Ok(manifest) = crate::manifest::Manifest::load(paths) {
            lobes.extend(
                manifest
                    .items
                    .values()
                    .filter_map(|i| i.lobe.as_deref())
                    .map(|l| normalize_home(Path::new(l))),
            );
        }
        Self {
            homes,
            stores,
            tmps,
            lobes,
        }
    }

    fn under(path: &Path, roots: &[&[PathBuf]]) -> bool {
        roots
            .iter()
            .flat_map(|r| r.iter())
            .any(|root| lexically_contained(path, root) && ancestors_confined(path, root))
    }

    fn check(&self, intent: &Intent) -> std::result::Result<(), String> {
        let outside = |p: &Path| {
            format!(
                "'{}' is outside every store, lobe and .tmp tree",
                p.display()
            )
        };
        match intent {
            // A transaction only moves within mind's own trees (staging into
            // the store) or into its trash (a store copy, a lobe entry); undo
            // moves back. A crafted journal must not move a mind-controlled
            // file into a lobe, nor have the commit delete in one.
            Intent::Move { from, to, .. } => {
                if !Self::under(from, &[&self.stores, &self.tmps, &self.lobes]) {
                    return Err(outside(from));
                }
                if !Self::under(to, &[&self.stores, &self.tmps]) {
                    return Err(format!(
                        "'{}' is outside every store and .tmp tree",
                        to.display()
                    ));
                }
                let into_trash = Self::under(to, &[&self.tmps]) && is_trash(to);
                if is_trash(to) && !into_trash {
                    return Err(format!(
                        "trash '{}' is outside every .tmp tree",
                        to.display()
                    ));
                }
                if !Self::under(from, &[&self.stores, &self.tmps]) && !into_trash {
                    return Err(format!(
                        "lobe entry '{}' may only move into a .tmp trash",
                        from.display()
                    ));
                }
                Ok(())
            }
            Intent::CreateLink { path, target } | Intent::RemoveLink { path, target } => {
                if !Self::under(path, &[&self.lobes]) {
                    return Err(format!("link '{}' is outside every lobe", path.display()));
                }
                // A relative MIND_HOME writes link targets relative to the
                // working directory, as the store path itself was.
                let target = normalize_home(target);
                if !self.stores.iter().any(|s| lexically_contained(&target, s)) {
                    return Err(format!(
                        "link target '{}' is outside every store",
                        target.display()
                    ));
                }
                Ok(())
            }
            Intent::Manifest {
                destination,
                contents,
                ..
            } => {
                if !self
                    .homes
                    .iter()
                    .any(|h| *destination == h.join("manifest.json"))
                {
                    return Err(format!(
                        "destination '{}' is not a configured home's manifest",
                        destination.display()
                    ));
                }
                crate::manifest::check_journal_payload(contents)
            }
            Intent::Hook { .. } | Intent::HookDone => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "mind-journal-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        path
    }

    // spec: LIFE-59
    #[test]
    fn journal_persists_intents_and_irreversible_decision() {
        let dir = dir();
        let file = dir.join("journal.json");
        let mut journal = Journal::begin(&file).unwrap();
        let intent = Intent::Move {
            from: dir.join("old"),
            to: dir.join("backup"),
            identity: Some((1, 2)),
        };
        journal.record(intent.clone()).unwrap();
        assert_eq!(Journal::load(&file).unwrap().intents, vec![intent]);
        journal.commit().unwrap();
        assert_eq!(Journal::load(&file).unwrap().decision, Decision::Commit);
        assert!(
            journal
                .record(Intent::Move {
                    from: dir.join("a"),
                    to: dir.join("b"),
                    identity: None,
                })
                .is_err()
        );
        assert!(Journal::begin(&file).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn malformed_or_unknown_journal_fails_closed() {
        let dir = dir();
        let file = dir.join("journal.json");
        fs::write(
            &file,
            b"{\"version\":2,\"decision\":\"Preparing\",\"intents\":[]}",
        )
        .unwrap();
        assert_eq!(
            Journal::load(&file).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        fs::write(&file, b"{").unwrap();
        assert_eq!(
            Journal::load(&file).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn oversized_intent_is_not_published_or_committed() {
        let dir = dir();
        let file = dir.join("journal.json");
        let mut journal = Journal::begin(&file).unwrap();
        let before = fs::read(&file).unwrap();
        let result = journal.record(Intent::Manifest {
            staged: dir.join("stage"),
            destination: dir.join("manifest"),
            contents: vec![b'0'; MAX_JOURNAL_BYTES / 3 + 1],
            prior: None,
        });
        assert!(result.is_err());
        assert_eq!(fs::read(&file).unwrap(), before);
        assert_eq!(Journal::load(&file).unwrap().decision, Decision::Preparing);
        fs::remove_dir_all(dir).unwrap();
    }

    fn moved(journal: &mut Journal, from: &Path, to: &Path) {
        journal
            .record(Intent::Move {
                from: from.to_path_buf(),
                to: to.to_path_buf(),
                identity: identity_of(from),
            })
            .unwrap();
        fs::rename(from, to).unwrap();
    }

    // spec: LIFE-59
    #[test]
    fn move_intent_can_be_undone_repeatedly() {
        let dir = dir();
        let file = dir.join("journal.json");
        let original = dir.join("original");
        let backup = dir.join("backup");
        fs::write(&original, b"original content").unwrap();
        let mut journal = Journal::begin(&file).unwrap();
        moved(&mut journal, &original, &backup);
        // Simulate a crash after the undo ran but before the journal shrank.
        undo(&journal.intents[0]).unwrap();
        let mut journal = Journal::load(&file).unwrap();
        journal.undo_to(0).unwrap();
        assert!(Journal::load(&file).unwrap().intents.is_empty());
        assert_eq!(fs::read(&original).unwrap(), b"original content");
        assert!(!backup.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    // spec: LIFE-59
    #[test]
    fn rollback_handles_overlapping_moves_after_restart() {
        let dir = dir();
        let file = dir.join("journal.json");
        let original = dir.join("original");
        let backup = dir.join("backup");
        let staging = dir.join("staging");
        fs::write(&original, b"old").unwrap();
        fs::write(&staging, b"new").unwrap();
        let mut journal = Journal::begin(&file).unwrap();
        moved(&mut journal, &original, &backup);
        moved(&mut journal, &staging, &original);
        // Crash after undoing the second move, before recording progress:
        // replay must infer that move was already reversed.
        fs::rename(&original, &staging).unwrap();
        Journal::load(&file).unwrap().undo_to(0).unwrap();
        assert_eq!(fs::read(&original).unwrap(), b"old");
        assert_eq!(fs::read(&staging).unwrap(), b"new");
        fs::remove_dir_all(dir).unwrap();
    }

    // spec: LIFE-59
    #[test]
    fn link_intents_undo_to_the_prior_link() {
        let dir = dir();
        let file = dir.join("journal.json");
        let old = dir.join("old-store");
        let new = dir.join("new-store");
        fs::create_dir(&old).unwrap();
        fs::create_dir(&new).unwrap();
        let link = dir.join("link");
        make_symlink(&old, &link).unwrap();
        let mut journal = Journal::begin(&file).unwrap();
        journal
            .record(Intent::RemoveLink {
                path: link.clone(),
                target: old.clone(),
            })
            .unwrap();
        fs::remove_file(&link).unwrap();
        journal
            .record(Intent::CreateLink {
                path: link.clone(),
                target: new.clone(),
            })
            .unwrap();
        make_symlink(&new, &link).unwrap();
        Journal::load(&file).unwrap().undo_to(0).unwrap();
        assert_eq!(fs::read_link(&link).unwrap(), old);
        // An occupied path is not the transaction's to clobber.
        fs::remove_file(&link).unwrap();
        fs::write(&link, b"user file").unwrap();
        let mut journal = Journal::begin(&dir.join("second.json")).unwrap();
        journal
            .record(Intent::RemoveLink {
                path: link.clone(),
                target: old.clone(),
            })
            .unwrap();
        assert!(journal.undo_to(0).is_err());
        assert_eq!(fs::read(&link).unwrap(), b"user file");
        fs::remove_dir_all(dir).unwrap();
    }

    // spec: LIFE-60
    #[test]
    fn committed_manifests_replay_after_partial_publication() {
        let dir = dir();
        let file = dir.join("journal.json");
        let first = dir.join("manifest-one");
        let second = dir.join("manifest-two");
        let mut journal = Journal::begin(&file).unwrap();
        for destination in [&first, &second] {
            journal
                .record(Intent::Manifest {
                    staged: destination.with_extension("stage"),
                    destination: destination.to_path_buf(),
                    contents: b"committed manifest".to_vec(),
                    prior: Some(Prior::Contents(crate::hash::fingerprint_bytes(
                        b"old manifest",
                    ))),
                })
                .unwrap();
        }
        journal.commit().unwrap();
        fs::write(&first, b"committed manifest").unwrap();
        fs::write(&second, b"old manifest").unwrap();
        let mut journal = Journal::load(&file).unwrap();
        journal.finish_prevalidated_manifests().unwrap();
        journal.finish_prevalidated_manifests().unwrap();
        assert_eq!(fs::read(first).unwrap(), b"committed manifest");
        assert_eq!(fs::read(second).unwrap(), b"committed manifest");
        assert!(journal.undo_to(0).is_err(), "a commit is never undone");
        fs::remove_dir_all(dir).unwrap();
    }

    // spec: LIFE-64
    #[test]
    fn a_manifest_changed_since_the_commit_is_not_overwritten() {
        let dir = dir();
        let file = dir.join("journal.json");
        let first = dir.join("manifest-one");
        let second = dir.join("manifest-two");
        fs::write(&first, b"old one").unwrap();
        fs::write(&second, b"old two").unwrap();
        let mut journal = Journal::begin(&file).unwrap();
        for destination in [&first, &second] {
            journal
                .record(Intent::Manifest {
                    staged: destination.clone(),
                    destination: destination.clone(),
                    contents: b"new".to_vec(),
                    prior: Some(Prior::of(destination).unwrap()),
                })
                .unwrap();
        }
        journal.commit().unwrap();
        // Another mind writes the second manifest before recovery.
        fs::write(&second, b"written by another mind").unwrap();
        let err = journal.finish_prevalidated_manifests().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("changed after"), "{err}");
        // Checked before publishing any: the first is untouched too.
        assert_eq!(fs::read(&first).unwrap(), b"old one");
        assert_eq!(fs::read(&second).unwrap(), b"written by another mind");
        // A destination missing when it had contents is a change as well.
        fs::write(&second, b"old two").unwrap();
        fs::remove_file(&first).unwrap();
        assert!(journal.finish_prevalidated_manifests().is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    // spec: LIFE-64
    #[cfg(unix)]
    #[test]
    fn a_rollback_stops_before_moving_an_entry_it_did_not_put_there() {
        let dir = dir();
        let file = dir.join("journal.json");
        let store = dir.join("store");
        let trash = dir.join("trash");
        let staging = dir.join("staging");
        let link = dir.join("link");
        fs::write(&store, b"old copy").unwrap();
        fs::write(&staging, b"new copy").unwrap();
        let mut journal = Journal::begin(&file).unwrap();
        moved(&mut journal, &store, &trash);
        moved(&mut journal, &staging, &store);
        journal
            .record(Intent::CreateLink {
                path: link.clone(),
                target: store.clone(),
            })
            .unwrap();
        make_symlink(&store, &link).unwrap();
        // Another mind reinstalls the store copy: a different entry.
        let theirs = dir.join("theirs");
        fs::write(&theirs, b"their copy").unwrap();
        fs::rename(&theirs, &store).unwrap();
        let mut journal = Journal::load(&file).unwrap();
        let err = journal.undo_to(0).unwrap_err();
        assert!(err.to_string().contains("no longer what"), "{err}");
        // Nothing was undone, not even the link undone first.
        assert_eq!(fs::read_link(&link).unwrap(), store);
        assert_eq!(fs::read(&store).unwrap(), b"their copy");
        assert_eq!(fs::read(&trash).unwrap(), b"old copy");
        assert_eq!(Journal::load(&file).unwrap().intents.len(), 3);
        fs::remove_dir_all(dir).unwrap();
    }

    // spec: LIFE-62
    #[test]
    fn unfinished_hooks_are_reported() {
        let mut journal = Journal::volatile();
        for intent in [
            Intent::Hook { label: "a".into() },
            Intent::HookDone,
            Intent::Hook { label: "b".into() },
        ] {
            journal.record(intent).unwrap();
        }
        assert_eq!(journal.interrupted_hooks(), vec!["b".to_string()]);
    }

    #[test]
    fn trash_entries_are_recognized_by_location() {
        assert!(is_trash(Path::new("/h/.tmp/tx/123-4/1")));
        assert!(!is_trash(Path::new("/h/store/skill/x")));
        assert!(!is_trash(Path::new("/h/.tmp/staging/skill/x")));
    }

    #[test]
    fn lexical_guard_rejects_escape_and_root_itself() {
        assert!(lexically_contained(
            Path::new("/home/a/store/item"),
            Path::new("/home/a/store")
        ));
        for path in [
            "/home/a/store",
            "/home/a/store/../secret",
            "/home/a/storehouse/item",
            "store/item",
        ] {
            assert!(!lexically_contained(
                Path::new(path),
                Path::new("/home/a/store")
            ));
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_ancestor_escapes_its_root() {
        let dir = dir();
        let root = dir.join("root");
        let outside = dir.join("outside");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        make_symlink(&outside, &root.join("hop")).unwrap();
        assert!(!ancestors_confined(&root.join("hop/x"), &root));
        // A missing directory below the symlink does not hide it.
        assert!(!ancestors_confined(&root.join("hop/missing/x"), &root));
        assert!(ancestors_confined(&root.join("x"), &root));
        assert!(ancestors_confined(&root.join("missing/x"), &root));
        fs::remove_dir_all(dir).unwrap();
    }

    fn roots(dir: &Path) -> Roots {
        Roots {
            homes: vec![dir.join("home")],
            stores: vec![dir.join("home/store")],
            tmps: vec![dir.join("home/.tmp")],
            lobes: vec![dir.join("lobe")],
        }
    }

    // spec: LIFE-61
    #[test]
    fn recovery_admits_only_the_moves_a_transaction_makes() {
        let dir = dir();
        let roots = roots(&dir);
        let mv = |from: &str, to: &str| Intent::Move {
            from: dir.join(from),
            to: dir.join(to),
            identity: None,
        };
        for (from, to) in [
            ("home/.tmp/staging/skill/a", "home/store/skill/a"),
            ("home/store/skill/a", "home/.tmp/tx/1-2/1"),
            ("lobe/skills/a", "home/.tmp/tx/1-2/2"),
        ] {
            assert!(roots.check(&mv(from, to)).is_ok(), "{from} -> {to}");
        }
        for (from, to) in [
            // Undo would move a mind-controlled file into a lobe.
            ("home/store/skill/a", "lobe/skills/a"),
            ("lobe/skills/a", "lobe/skills/b"),
            // A lobe entry outside the trash would not be restored by undo.
            ("lobe/skills/a", "home/store/skill/a"),
            // A commit deletes trash: never in a store or a lobe.
            ("home/store/skill/a", "home/store/.tmp/tx/1-2/1"),
            ("home/store/skill/a", "outside/x"),
        ] {
            assert!(roots.check(&mv(from, to)).is_err(), "{from} -> {to}");
        }
        fs::remove_dir_all(dir).unwrap();
    }

    // spec: LIFE-59
    #[test]
    fn a_failed_rename_is_forgotten_and_rolls_back_cleanly() {
        let dir = dir();
        let (from, to) = (dir.join("from"), dir.join("to"));
        fs::create_dir(&from).unwrap();
        fs::create_dir(&to).unwrap();
        fs::write(to.join("occupied"), b"x").unwrap();
        let scope = scope();
        // Renaming a directory onto a non-empty one fails, changing nothing.
        assert!(move_path(&from, &to).is_err());
        assert_eq!(active().as_ref().unwrap().len(), 0);
        drop(scope);
        assert!(active().is_none());
        assert!(from.exists() && to.join("occupied").exists());
        fs::remove_dir_all(dir).unwrap();
    }
}
