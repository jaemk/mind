//! Dependency-free content hashing for drift / upgrade detection.
//!
//! Uses FNV-1a (64-bit) over file contents, plus the relative path of each
//! file so renames register as changes. This is not cryptographic; it only
//! needs to be stable and collision-resistant enough to tell "changed" from
//! "unchanged".

use std::path::Path;

use crate::error::{MindError, Result};

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

#[derive(Clone, Copy)]
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Fnv(FNV_OFFSET)
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= b as u64;
            self.0 = self.0.wrapping_mul(FNV_PRIME);
        }
    }
    fn finish_hex(self) -> String {
        format!("{:016x}", self.0)
    }
    /// The raw digest, for folding one hash into another as a field.
    fn value(self) -> u64 {
        self.0
    }
}

/// Hash an arbitrary string, returning its 16-hex-digit FNV-1a digest.
///
/// Used where a value needs to be folded down to a short, fixed-length,
/// filesystem-safe token rather than embedded verbatim -- e.g. the
/// clone-dir leaf falls back to this when the readable, percent-encoded
/// `item_path` segment would push the leaf past a safe length (STO-70).
/// Not cryptographic (see the module doc); only needs to be stable and
/// collision-resistant enough for that purpose.
pub(crate) fn hash_str(s: &str) -> String {
    let mut h = Fnv::new();
    h.write(s.as_bytes());
    h.finish_hex()
}

/// Fingerprint a file's bytes as `<length>:<FNV-1a digest>`: enough to tell
/// whether a file still holds what it did (LIFE-64), not cryptographic.
pub(crate) fn fingerprint_bytes(bytes: &[u8]) -> String {
    let mut h = Fnv::new();
    h.write(bytes);
    format!("{}:{}", bytes.len(), h.finish_hex())
}

/// Hash an item path: a single file, or a directory hashed recursively, with
/// NO ignore rules applied -- not even the IGN-2 built-ins (`.git`, `.hg`,
/// `.svn`, `.bzr`): it calls [`hash_path_ignoring`] with
/// `IgnoreSet::default()`, which is an empty rule set and is NOT the same as
/// [`crate::ignore::IgnoreSet::builtin()`].
///
/// NEVER compare this result against a recorded manifest hash: the manifest
/// hash was computed with the item's own effective ignore set (its declared
/// patterns plus the IGN-2 built-ins), so a directory containing anything
/// those rules would exclude -- most commonly `.git/`, which every
/// `path = "."` item has -- hashes differently here forever, and the
/// comparison reports permanent, unfixable drift (H1). Prefer
/// [`crate::catalog::CatalogItem::content_hash`], which cannot be called
/// without the item's own ignore set, for any comparison against a manifest
/// hash. This is `pub(crate)`, not `pub`: the one remaining caller
/// (`review.rs`) hashes single files it already knows carry no ignorable
/// content, never a value compared against a recorded hash.
///
/// Symlinks are never followed (LIFE-34): a symlink entry is hashed by its
/// relative path and link-target string, so retargeting is detected and a
/// symlink cycle cannot cause unbounded recursion.
///
/// Framing (LIFE-35): every field is length-prefixed (8-byte LE u64) and each
/// entry carries a 1-byte type tag (`b'F'` for file, `b'S'` for symlink).
/// This prevents distinct `(path, content)` pairs from colliding due to
/// ambiguous byte boundaries, and prevents a regular file whose name begins
/// with "symlink:" from producing the same hash as a symlink of that stem.
pub(crate) fn hash_path(path: &Path) -> Result<String> {
    hash_path_ignoring(path, &crate::ignore::IgnoreSet::default())
}

/// [`hash_path`], excluding what `ignore` matches (IGN-10).
///
/// This is the hash an ITEM is measured by, and it must agree exactly with what
/// the install copy wrote: a file hashed but not installed makes `upgrade`
/// offer a change the user cannot see in the installed item, and a file
/// installed but not hashed makes a change to it invisible to drift detection.
/// Prefer `CatalogItem::content_hash`, which cannot be called without the
/// item's own set.
pub fn hash_path_ignoring(path: &Path, ignore: &crate::ignore::IgnoreSet) -> Result<String> {
    let mut h = Fnv::new();
    let meta = std::fs::symlink_metadata(path).map_err(|e| MindError::io(path, e))?;
    if meta.file_type().is_symlink() {
        // Type tag + length-prefixed target so the symlink hash is always
        // distinct from a regular file whose raw bytes happen to match.
        // spec: LIFE-35
        let target = std::fs::read_link(path).map_err(|e| MindError::io(path, e))?;
        let target_bytes = target.to_string_lossy();
        h.write(b"S");
        h.write(&(target_bytes.len() as u64).to_le_bytes());
        h.write(target_bytes.as_bytes());
    } else if meta.is_dir() {
        let mut files = Vec::new();
        collect_files(path, path, &mut files, ignore)?;
        files.sort_by(entry_order);
        // spec: LIFE-35 - length-prefixed fields prevent (path, content) split
        // collisions across entries.
        let mut buf = vec![0u8; READ_BUF];
        for (tag, rel, content) in files {
            h.write(&[tag]);
            h.write(&(rel.len() as u64).to_le_bytes());
            h.write(rel.as_bytes());
            match content {
                Content::Link(bytes) => {
                    h.write(&(bytes.len() as u64).to_le_bytes());
                    h.write(&bytes);
                }
                Content::File(file) => write_framed_file(&mut h, &file, &mut buf)?,
            }
        }
    } else {
        // Plain file: type tag + raw content. No length-prefix on content needed
        // for single-file hashes since there is only one field; the type tag
        // still distinguishes the file hash from any symlink hash.
        // spec: LIFE-35 LIFE-65
        if !meta.is_file() {
            return Err(crate::install::not_regular_file(path));
        }
        let mut file = crate::install::open_regular(path)?;
        h.write(b"F");
        stream_into(&mut h, &mut file, path, &mut vec![0u8; READ_BUF])?;
    }
    Ok(h.finish_hex())
}

/// Test-only convenience: [`stat_fingerprint_ignoring`] with an empty
/// (`IgnoreSet::default()`) rule set. See that function's doc for the full
/// rationale; the shipping code always calls the ignoring form so it sees the
/// same tree the recorded hash does (IGN-10).
#[cfg(test)]
pub(crate) fn stat_fingerprint(path: &Path) -> Result<String> {
    stat_fingerprint_ignoring(path, &crate::ignore::IgnoreSet::default())
}

/// A cheap change-detection fingerprint for the same tree [`hash_path_ignoring`]
/// hashes: the identical walk, but reading each entry's stat fields -- `mtime`,
/// `size`, and, under `cfg(unix)`, `ctime` and inode number -- instead of its
/// contents. Symlinks are still not followed and still contribute their target
/// string (which is cheap and catches a retarget).
///
/// This is NOT a content hash and must never be compared against a recorded
/// manifest hash. Its only use is to decide whether re-reading content is
/// warranted: equal fingerprint means "nothing observably changed, reuse the
/// previous content hash". The real blind spot is not filesystem mtime
/// granularity: it is any mtime-PRESERVING replacement at unchanged size --
/// `cp -p`, `rsync -a`, `tar -p`, `touch -r`, some FUSE/network mounts -- which
/// leaves `(mtime, size)` alone regardless of granularity. Folding in `ctime`
/// and the inode number under `cfg(unix)` closes this for the realistic cases:
/// `ctime` cannot be set from userland and changes on every write (even one
/// that deliberately preserves `mtime`), and those tools' usual "preserve
/// mtime" replace (write a new file, then rename it over the old path) swaps
/// the inode too. This is why the only caller is the TUI's display-side
/// staleness memo (tui/data.rs, TUI-72); every path that ACTS on a hash
/// (`upgrade`, `introspect`, `recall`) calls `CatalogItem::content_hash`
/// (which calls `hash_path_ignoring` with the item's own ignore set) and is
/// unaffected by any residual gap here.
///
/// The fingerprint gates whether the TUI re-hashes (TUI-72), so it has to see
/// the same tree the hash does: a fingerprint that noticed an ignored file
/// would drive a re-hash that always returns the same value, and one that
/// missed a real change would suppress a re-hash that mattered.
pub(crate) fn stat_fingerprint_ignoring(
    path: &Path,
    ignore: &crate::ignore::IgnoreSet,
) -> Result<String> {
    let mut h = Fnv::new();
    let meta = std::fs::symlink_metadata(path).map_err(|e| MindError::io(path, e))?;
    if meta.file_type().is_symlink() {
        let target = std::fs::read_link(path).map_err(|e| MindError::io(path, e))?;
        let target_bytes = target.to_string_lossy();
        h.write(b"S");
        h.write(&(target_bytes.len() as u64).to_le_bytes());
        h.write(target_bytes.as_bytes());
    } else if meta.is_dir() {
        let mut entries = Vec::new();
        collect_stats(path, path, &mut entries, 0, ignore)?;
        entries.sort();
        for (tag, rel, mtime, size, ino, ctime) in entries {
            h.write(&[tag]);
            h.write(&(rel.len() as u64).to_le_bytes());
            h.write(rel.as_bytes());
            h.write(&mtime.to_le_bytes());
            h.write(&size.to_le_bytes());
            h.write(&ino.to_le_bytes());
            h.write(&ctime.to_le_bytes());
        }
    } else {
        let (ino, ctime) = unix_stat_fields(&meta);
        h.write(b"F");
        h.write(&mtime_nanos(&meta, path)?.to_le_bytes());
        h.write(&meta.len().to_le_bytes());
        h.write(&ino.to_le_bytes());
        h.write(&ctime.to_le_bytes());
    }
    Ok(h.finish_hex())
}

/// `(inode, ctime_nanos)` for a stat entry, under `cfg(unix)`
/// (`std::os::unix::fs::MetadataExt`). `ctime` cannot be set from userland --
/// it is bumped by the kernel on any inode metadata change, including a write
/// that deliberately preserves `mtime` -- so it closes `stat_fingerprint`'s
/// realistic blind spot (see its doc). Always `(0, 0)` on a non-unix target, so
/// the fingerprint still compiles and functions there, just without this extra
/// coverage.
#[cfg(unix)]
fn unix_stat_fields(meta: &std::fs::Metadata) -> (u64, i128) {
    use std::os::unix::fs::MetadataExt;
    let ctime_nanos = meta.ctime() as i128 * 1_000_000_000 + meta.ctime_nsec() as i128;
    (meta.ino(), ctime_nanos)
}

#[cfg(not(unix))]
fn unix_stat_fields(_meta: &std::fs::Metadata) -> (u64, i128) {
    (0, 0)
}

/// Modification time as signed nanoseconds since the epoch. Signed (not the
/// previous `u128` with `unwrap_or(0)`) so a pre-epoch mtime keeps its own
/// distinct value instead of every pre-epoch timestamp collapsing to the same
/// constant, which would make two edits between two pre-epoch mtimes invisible
/// to the fingerprint. An error (a platform or filesystem that does not report
/// mtime) propagates tagged with the real file path -- not a placeholder -- so
/// the caller falls back to a real content hash rather than trusting a
/// constant fingerprint.
fn mtime_nanos(meta: &std::fs::Metadata, path: &Path) -> Result<i128> {
    let t = meta.modified().map_err(|e| MindError::io(path, e))?;
    Ok(match t.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_nanos() as i128,
        Err(e) => -(e.duration().as_nanos() as i128),
    })
}

/// Walk `dir` and collect `(type_tag, relative_path, mtime_nanos, size, inode,
/// ctime_nanos)` tuples: the [`collect_files`] walk with `stat` in place of
/// `read`. For a symlink entry the `size`/`ctime` slots double as a folded
/// target-string digest / zero (safe because the type tag is hashed first, so
/// a symlink entry can never be mistaken for a file entry regardless of what
/// numbers land in those slots). Depth-capped like [`collect_files_at`]
/// (LIFE-52), so a pathological tree is an error here too, not a stack overflow.
fn collect_stats(
    root: &Path,
    dir: &Path,
    out: &mut Vec<(u8, String, i128, u64, u64, i128)>,
    depth: usize,
    ignore: &crate::ignore::IgnoreSet,
) -> Result<()> {
    if depth > crate::install::MAX_ITEM_TREE_DEPTH {
        return Err(crate::install::depth_exceeded(dir));
    }
    let rd = std::fs::read_dir(dir).map_err(|e| MindError::io(dir, e))?;
    for entry in rd {
        let entry = entry.map_err(|e| MindError::io(dir, e))?;
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path).map_err(|e| MindError::io(&path, e))?;
        let ft = meta.file_type();
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        // spec: IGN-10 -- the fingerprint sees exactly the tree the hash does.
        if ignore.is_ignored(path.strip_prefix(root).unwrap_or(&path), ft.is_dir()) {
            continue;
        }
        if ft.is_symlink() {
            // Hash the target string, not a stat: a retarget changes it even when
            // the link's own mtime does not.
            let target = std::fs::read_link(&path).map_err(|e| MindError::io(&path, e))?;
            let target_bytes = target.to_string_lossy();
            let mut th = Fnv::new();
            th.write(target_bytes.as_bytes());
            out.push((b'S', rel, 0, th.value(), 0, 0));
        } else if ft.is_dir() {
            collect_stats(root, &path, out, depth + 1, ignore)?;
        } else {
            let (ino, ctime) = unix_stat_fields(&meta);
            out.push((
                b'F',
                rel,
                mtime_nanos(&meta, &path)?,
                meta.len(),
                ino,
                ctime,
            ));
        }
    }
    Ok(())
}

/// What one hashed entry contributes as its content field.
enum Content {
    /// A symlink's target string, which is small and read during the walk.
    Link(Vec<u8>),
    /// A regular file, streamed from this path when it is hashed.
    File(std::path::PathBuf),
}

/// One hashed entry: `(type_tag, relative_path_string, content)`.
type Entry = (u8, String, Content);

/// The order entries are hashed in: by type tag (every file before every
/// symlink), then relative path. That is the order the earlier sort over whole
/// `(tag, path, content)` triples produced, so hash values are unchanged.
///
/// The path is the lossy UTF-8 string, so two non-UTF-8 names can share it
/// (`a\xff` and `a\xfe` both read `a\u{FFFD}`). The tie is broken
/// deterministically, never left to `read_dir` order: a symlink by its target,
/// as the content sort did, and a file by its raw path bytes, which avoids
/// reading both files to compare them.
fn entry_order(a: &Entry, b: &Entry) -> std::cmp::Ordering {
    (a.0, &a.1)
        .cmp(&(b.0, &b.1))
        .then_with(|| match (&a.2, &b.2) {
            (Content::Link(x), Content::Link(y)) => x.cmp(y),
            (Content::File(x), Content::File(y)) => x.as_os_str().cmp(y.as_os_str()),
            // Equal tags imply equal variants: `S` is always a link, `F` a file.
            _ => std::cmp::Ordering::Equal,
        })
}

/// The read buffer for streaming a file into the hash.
const READ_BUF: usize = 64 * 1024;

/// Stream `reader` into `h` through `buf`, returning the number of bytes read.
fn stream_into(
    h: &mut Fnv,
    reader: &mut impl std::io::Read,
    path: &Path,
    buf: &mut [u8],
) -> Result<u64> {
    let mut total = 0u64;
    loop {
        match reader.read(buf) {
            Ok(0) => return Ok(total),
            Ok(n) => {
                h.write(&buf[..n]);
                total += n as u64;
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(MindError::io(path, e)),
        }
    }
}

/// Write a file's length-prefixed content field (LIFE-35) into `h`, streaming
/// the content rather than holding it.
///
/// The length goes into the hash before the content, so it is taken from the
/// open file's metadata up front and checked against the bytes actually read.
/// A mismatch means the file changed during the read: the hash is rolled back
/// and the file read once more, and a second mismatch is an error naming the
/// file, never a hash of a half-old, half-new file.
fn write_framed_file(h: &mut Fnv, path: &Path, buf: &mut [u8]) -> Result<()> {
    write_framed_with(h, path, buf, || {
        let file = crate::install::open_regular(path)?;
        let len = file.metadata().map_err(|e| MindError::io(path, e))?.len();
        Ok((len, file))
    })
}

/// [`write_framed_file`] over any opener yielding `(stated length, reader)`,
/// so the changed-during-read path can be driven without a real race. The read
/// stops one byte past the stated length: that is enough to see the file grew,
/// without reading the rest of a file that keeps growing.
fn write_framed_with<R: std::io::Read>(
    h: &mut Fnv,
    path: &Path,
    buf: &mut [u8],
    mut open: impl FnMut() -> Result<(u64, R)>,
) -> Result<()> {
    for _ in 0..2 {
        let mut attempt = *h;
        let (len, reader) = open()?;
        attempt.write(&len.to_le_bytes());
        let mut reader = std::io::Read::take(reader, len.saturating_add(1));
        if stream_into(&mut attempt, &mut reader, path, buf)? == len {
            *h = attempt;
            return Ok(());
        }
    }
    Err(MindError::io(
        path,
        std::io::Error::other("the file changed while it was being hashed"),
    ))
}

/// Walk `dir` and collect one [`Entry`] per symlink and regular file.
///
/// Uses `symlink_metadata` at every step so symlinks are never followed
/// (LIFE-34). A symlink entry carries type tag `b'S'` and contributes its
/// link-target string as its content; a regular file carries `b'F'`. The
/// separate type tag prevents a file named `"symlink:foo"` from producing the
/// same triple as a symlink named `"foo"` (LIFE-35). A file's content is not
/// read here: the caller streams it after sorting, so a walk holds paths, not
/// file contents.
fn collect_files(
    root: &Path,
    dir: &Path,
    out: &mut Vec<Entry>,
    ignore: &crate::ignore::IgnoreSet,
) -> Result<()> {
    collect_files_at(root, dir, out, 0, ignore)
}

// spec: LIFE-52 -- depth-capped like install.rs's walks; the not-followed
// symlinks (LIFE-34) already prevent cycle recursion, the cap covers plain
// deep nesting.
fn collect_files_at(
    root: &Path,
    dir: &Path,
    out: &mut Vec<Entry>,
    depth: usize,
    ignore: &crate::ignore::IgnoreSet,
) -> Result<()> {
    if depth > crate::install::MAX_ITEM_TREE_DEPTH {
        return Err(crate::install::depth_exceeded(dir));
    }
    let rd = std::fs::read_dir(dir).map_err(|e| MindError::io(dir, e))?;
    for entry in rd {
        let entry = entry.map_err(|e| MindError::io(dir, e))?;
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path).map_err(|e| MindError::io(&path, e))?;
        let ft = meta.file_type();
        // spec: IGN-10 -- the same exclusion the install copy applies, so what
        // is not installed is not hashed. A matching directory is skipped whole
        // and never descended into.
        if ignore.is_ignored(path.strip_prefix(root).unwrap_or(&path), ft.is_dir()) {
            continue;
        }
        if ft.is_symlink() {
            // spec: LIFE-34 LIFE-35
            let target = std::fs::read_link(&path).map_err(|e| MindError::io(&path, e))?;
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            out.push((
                b'S',
                rel,
                Content::Link(target.to_string_lossy().into_owned().into_bytes()),
            ));
        } else if ft.is_dir() {
            collect_files_at(root, &path, out, depth + 1, ignore)?;
        } else {
            // spec: LIFE-35 LIFE-65
            if !ft.is_file() {
                return Err(crate::install::not_regular_file(&path));
            }
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            out.push((b'F', rel, Content::File(path)));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A temp dir that removes itself on drop, so each test self-cleans even
    /// when an assertion panics (Drop runs during unwinding). Derefs to the dir
    /// `Path` so existing `&dir` / `dir.join(..)` call sites are unchanged.
    struct TmpDir(std::path::PathBuf);

    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    impl std::ops::Deref for TmpDir {
        type Target = std::path::Path;
        fn deref(&self) -> &std::path::Path {
            &self.0
        }
    }

    fn tmp(name: &str) -> TmpDir {
        let dir = std::env::temp_dir().join(format!("mind-hashtest-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TmpDir(dir)
    }

    /// Create a FIFO at `p`. Opening one for reading blocks until a writer
    /// appears, which is the hang LIFE-65 guards against.
    #[cfg(unix)]
    fn mkfifo(p: &Path) {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(p.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0, "mkfifo");
    }

    /// Run `f` under a deadline so a hang fails the test instead of the suite.
    #[cfg(unix)]
    fn bounded<T: Send + 'static>(what: &str, f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(std::time::Duration::from_secs(10))
            .unwrap_or_else(|_| panic!("{what} hung"))
    }

    // spec: LIFE-65
    #[cfg(unix)]
    #[test]
    fn a_fifo_in_an_item_tree_is_refused_not_opened() {
        let dir = tmp("fifo-tree");
        std::fs::write(dir.join("SKILL.md"), b"x").unwrap();
        mkfifo(&dir.join("pipe"));
        let root = dir.to_path_buf();
        let err = bounded("hashing a tree holding a FIFO", move || {
            hash_path_ignoring(&root, &crate::ignore::IgnoreSet::builtin())
        })
        .expect_err("a FIFO must be refused");
        let msg = err.to_string();
        assert!(msg.contains("pipe"), "the error names the entry: {msg}");
        assert!(msg.contains("not a regular file"), "{msg}");
        // An ignored FIFO is never looked at, like any ignored entry (IGN-10).
        let ignore = crate::ignore::IgnoreSet::new(&["pipe".to_string()], "skill:t").unwrap();
        let root = dir.to_path_buf();
        bounded("hashing with the FIFO ignored", move || {
            hash_path_ignoring(&root, &ignore)
        })
        .expect("an ignored FIFO is skipped");
    }

    // spec: LIFE-65
    #[cfg(unix)]
    #[test]
    fn a_fifo_as_the_item_path_is_refused_not_opened() {
        let dir = tmp("fifo-item");
        let pipe = dir.join("rule.md");
        mkfifo(&pipe);
        let err = bounded("hashing a FIFO item", move || hash_path(&pipe))
            .expect_err("a FIFO item must be refused");
        assert!(err.to_string().contains("not a regular file"), "{err}");
    }

    /// A fixed tree covering every entry kind the hash frames: an empty file, a
    /// small file, a file larger than several read buffers, a nested
    /// directory, and (on unix) a symlink.
    fn golden_tree(dir: &Path) {
        std::fs::write(dir.join("empty.txt"), b"").unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            b"---\ndescription: golden\n---\nbody\n",
        )
        .unwrap();
        let big: Vec<u8> = (0..300_000u32).map(|i| (i * 31 % 251) as u8).collect();
        std::fs::create_dir_all(dir.join("sub").join("deeper")).unwrap();
        std::fs::write(dir.join("sub").join("big.bin"), &big).unwrap();
        std::fs::write(dir.join("sub").join("deeper").join("x"), b"x").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("sub/deeper/x", dir.join("link")).unwrap();
    }

    // spec: LIFE-35 -- the hash's byte stream is pinned: these are the values
    // the whole-file implementation produced before the walk was streamed, so
    // a change to the framing or the read path that would make every recorded
    // manifest hash read as drift fails here instead.
    #[test]
    fn hash_values_are_pinned() {
        let dir = tmp("golden");
        golden_tree(&dir);
        let tree = hash_path_ignoring(&dir, &crate::ignore::IgnoreSet::builtin()).unwrap();
        let file = hash_path(&dir.join("sub").join("big.bin")).unwrap();
        #[cfg(unix)]
        assert_eq!(tree, "34d773935e8a118f", "tree hash");
        assert_eq!(file, "18f801807acd8e9b", "single-file hash");
    }

    /// The digest `write_framed_with` should leave for `content` under a
    /// correct stated length: the length prefix, then the bytes.
    fn framed_digest(content: &[u8]) -> u64 {
        let mut h = Fnv::new();
        h.write(&(content.len() as u64).to_le_bytes());
        h.write(content);
        h.value()
    }

    // spec: LIFE-35 -- a file whose size changed between the stat and the
    // read is re-read once; the retry's hash replaces the first attempt's
    // entirely, so the result is exactly the stable file's framing.
    #[test]
    fn a_file_that_changes_once_mid_hash_is_re_read() {
        let mut calls = 0;
        let mut h = Fnv::new();
        write_framed_with(&mut h, Path::new("f"), &mut [0u8; 4], || {
            calls += 1;
            // First open: stated 3 bytes, reads 5 (it grew). Then stable.
            let (len, bytes): (u64, &[u8]) = match calls {
                1 => (3, b"hello"),
                _ => (5, b"hello"),
            };
            Ok((len, bytes))
        })
        .expect("the retry reads a stable file");
        assert_eq!(calls, 2);
        assert_eq!(h.value(), framed_digest(b"hello"));
    }

    // spec: LIFE-35 -- two names that read the same once made lossy UTF-8 are
    // ordered by their raw bytes, so the hash does not depend on the order the
    // directory listing returns them in.
    #[test]
    fn entries_with_the_same_lossy_path_order_deterministically() {
        let file = |raw: &str| (b'F', "a\u{FFFD}".to_string(), Content::File(raw.into()));
        let link = |t: &[u8]| (b'S', "a\u{FFFD}".to_string(), Content::Link(t.to_vec()));
        let names = |v: &[Entry]| -> Vec<String> {
            v.iter()
                .map(|e| match &e.2 {
                    Content::File(p) => p.to_string_lossy().into_owned(),
                    Content::Link(t) => String::from_utf8_lossy(t).into_owned(),
                })
                .collect()
        };
        let mut fwd = vec![file("/r/a2"), file("/r/a1"), link(b"y"), link(b"x")];
        let mut rev = vec![link(b"x"), link(b"y"), file("/r/a1"), file("/r/a2")];
        fwd.sort_by(entry_order);
        rev.sort_by(entry_order);
        assert_eq!(names(&fwd), ["/r/a1", "/r/a2", "x", "y"]);
        assert_eq!(names(&fwd), names(&rev));
    }

    // spec: LIFE-35 -- the same tree of non-UTF-8 names hashes the same
    // whatever order its entries were created (and so listed) in.
    #[cfg(unix)]
    #[test]
    fn non_utf8_names_hash_the_same_in_any_creation_order() {
        use std::os::unix::ffi::OsStrExt;
        let make = |label: &str, order: [&[u8]; 2]| {
            let dir = tmp(label);
            for raw in order {
                let name = std::ffi::OsStr::from_bytes(raw);
                std::fs::write(dir.join(name), raw).unwrap();
            }
            hash_path(&dir).unwrap()
        };
        let a = make("lossy-fwd", [b"a\xff", b"a\xfe"]);
        let b = make("lossy-rev", [b"a\xfe", b"a\xff"]);
        assert_eq!(a, b);
    }

    // spec: LIFE-35 -- a file that keeps growing is read only one byte past
    // its stated length, so an endless writer cannot keep the hash reading.
    #[test]
    fn a_file_that_keeps_growing_is_not_read_to_the_end() {
        let mut h = Fnv::new();
        let err = write_framed_with(&mut h, Path::new("grow.log"), &mut [0u8; 64], || {
            Ok::<_, MindError>((5u64, std::io::repeat(b'x')))
        })
        .expect_err("an endless file is refused, not read forever");
        assert!(
            err.to_string()
                .contains("changed while it was being hashed")
        );
    }

    // spec: LIFE-35 -- a file that keeps changing is an error naming it, never
    // a hash of a torn read.
    #[test]
    fn a_file_that_keeps_changing_mid_hash_is_an_error() {
        let mut h = Fnv::new();
        let before = h.value();
        let err = write_framed_with(&mut h, Path::new("busy.bin"), &mut [0u8; 4], || {
            Ok::<_, MindError>((10u64, &b"short"[..]))
        })
        .expect_err("a persistent mismatch is refused");
        let msg = err.to_string();
        assert!(msg.contains("busy.bin"), "{msg}");
        assert!(msg.contains("changed while it was being hashed"), "{msg}");
        assert_eq!(
            h.value(),
            before,
            "a failed attempt leaves the hash untouched"
        );
    }

    // spec: STO-70
    #[test]
    fn hash_str_is_stable_16_hex_and_distinguishes_input() {
        let a = hash_str("skills/foo");
        let b = hash_str("skills/foo");
        let c = hash_str("skills/bar");
        assert_eq!(a, b, "hash_str must be deterministic for the same input");
        assert_ne!(a, c, "distinct inputs must (in practice) hash differently");
        assert_eq!(a.len(), 16, "hash_str must return a 16-hex-digit digest");
        assert!(
            a.chars().all(|ch| ch.is_ascii_hexdigit()),
            "hash_str digest must be pure hex: {a}"
        );
    }

    // spec: TUI-72
    #[test]
    fn stat_fingerprint_is_stable_and_changes_with_content() {
        // The fingerprint gates whether the TUI re-hashes content, so it must be
        // stable across repeated calls over an untouched tree, and must change
        // when a file's content changes (here also changing its size, the case
        // the fingerprint is designed to catch).
        let dir = tmp("fingerprint");
        std::fs::write(dir.join("a.txt"), b"hello").unwrap();
        let f1 = stat_fingerprint(&dir).unwrap();
        let f2 = stat_fingerprint(&dir).unwrap();
        assert_eq!(f1, f2, "fingerprint must be stable for an untouched tree");

        std::fs::write(dir.join("a.txt"), b"hello world, longer").unwrap();
        let f3 = stat_fingerprint(&dir).unwrap();
        assert_ne!(f1, f3, "a content+size change must change the fingerprint");

        // A NEW file changes it too (the walk covers the whole tree).
        std::fs::write(dir.join("b.txt"), b"x").unwrap();
        let f4 = stat_fingerprint(&dir).unwrap();
        assert_ne!(f3, f4, "adding a file must change the fingerprint");

        // Removing it again returns to the previous fingerprint's shape (a
        // different mtime is possible, so just assert it moved off f4).
        std::fs::remove_file(dir.join("b.txt")).unwrap();
        let f5 = stat_fingerprint(&dir).unwrap();
        assert_ne!(f4, f5, "removing a file must change the fingerprint");
    }

    // spec: TUI-72
    #[test]
    fn stat_fingerprint_covers_nested_files_and_symlink_targets() {
        // A nested edit must be visible: caching on the ROOT directory's mtime
        // alone would miss this, which is why the fingerprint walks the tree.
        let dir = tmp("fingerprint-nested");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/deep.txt"), b"one").unwrap();
        let before = stat_fingerprint(&dir).unwrap();
        std::fs::write(dir.join("sub/deep.txt"), b"two different").unwrap();
        assert_ne!(
            before,
            stat_fingerprint(&dir).unwrap(),
            "a nested file edit must change the fingerprint"
        );

        // A symlink retarget changes the fingerprint even though its own stat
        // need not: the target string is folded in (matching hash_path, LIFE-34).
        #[cfg(unix)]
        {
            let link = dir.join("link");
            std::os::unix::fs::symlink("first-target", &link).unwrap();
            let with_first = stat_fingerprint(&dir).unwrap();
            std::fs::remove_file(&link).unwrap();
            std::os::unix::fs::symlink("second-target", &link).unwrap();
            assert_ne!(
                with_first,
                stat_fingerprint(&dir).unwrap(),
                "a symlink retarget must change the fingerprint"
            );
        }
    }

    // spec: TUI-72 IGN-10
    #[test]
    fn stat_fingerprint_ignoring_skips_ignored_dir_and_still_sees_tracked_changes() {
        // M16-a: every other fingerprint test in this module calls the walk
        // with `IgnoreSet::default()` (zero rules), so none of them ever
        // exercise `stat_fingerprint_ignoring`'s `ignore.is_ignored(..)` filter
        // (collect_stats) with a rule that actually MATCHES. A regression that
        // deleted that filter would pass every existing test here while
        // reintroducing a permanent ~1 Hz full-tree rehash for any item
        // containing a `.git` dir -- the exact cost TUI-72 exists to avoid.
        let dir = tmp("fingerprint-ignoring-git");
        std::fs::write(dir.join("SKILL.md"), b"tracked v1").unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join(".git").join("HEAD"), b"ref: refs/heads/main").unwrap();

        let ignore = crate::ignore::IgnoreSet::builtin();
        let before = stat_fingerprint_ignoring(&dir, &ignore).unwrap();

        // A mutation INSIDE the ignored `.git` dir must NOT move the
        // fingerprint: it is excluded from the walk entirely.
        std::fs::write(dir.join(".git").join("HEAD"), b"ref: refs/heads/other").unwrap();
        let after_git_edit = stat_fingerprint_ignoring(&dir, &ignore).unwrap();
        assert_eq!(
            before, after_git_edit,
            "a change inside an ignored .git dir must not move the fingerprint"
        );

        // A mutation to a TRACKED file must still move the fingerprint.
        std::fs::write(dir.join("SKILL.md"), b"tracked v2, different").unwrap();
        let after_tracked_edit = stat_fingerprint_ignoring(&dir, &ignore).unwrap();
        assert_ne!(
            after_git_edit, after_tracked_edit,
            "a change to a tracked file must still move the fingerprint"
        );
    }

    // spec: TUI-72
    #[test]
    fn stat_fingerprint_detects_same_size_content_change() {
        // M3(d): every prior fingerprint test above also changed the file's
        // SIZE alongside its content, so a fingerprint keyed on `size` alone
        // (with `mtime` silently dropped by a future edit) would still have
        // passed those tests. Isolate a same-size rewrite.
        let dir = tmp("fingerprint-same-size");
        std::fs::write(dir.join("a.txt"), b"aaaaa").unwrap();
        let f1 = stat_fingerprint(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"bbbbb").unwrap();
        let f2 = stat_fingerprint(&dir).unwrap();
        assert_ne!(
            f1, f2,
            "a same-size content rewrite must change the fingerprint"
        );
    }

    /// Force `path`'s mtime to an exact `(seconds, nanos)` value since the
    /// epoch, to simulate an mtime-PRESERVING replace (`cp -p`, `rsync -a`,
    /// `touch -r`) precisely rather than relying on two writes racing the
    /// clock. `ctime` cannot be forced this way (or any way, from userland):
    /// the kernel always bumps it to "now" on the `utimensat` call itself, so
    /// this helper is exactly the tool that would defeat a fingerprint keyed
    /// on `(mtime, size)` alone.
    #[cfg(unix)]
    fn force_mtime(path: &std::path::Path, seconds: i64, nanos: i64) {
        use std::ffi::CString;
        let c_path = CString::new(path.to_str().unwrap()).unwrap();
        let times = [
            libc::timespec {
                tv_sec: seconds,
                tv_nsec: nanos,
            },
            libc::timespec {
                tv_sec: seconds,
                tv_nsec: nanos,
            },
        ];
        let rc = unsafe { libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), times.as_ptr(), 0) };
        assert_eq!(rc, 0, "utimensat must succeed to force mtime for this test");
    }

    // spec: TUI-72
    #[cfg(unix)]
    #[test]
    fn stat_fingerprint_same_size_mtime_preserving_rewrite_still_changes() {
        // M3(a)/(c): the realistic blind spot is not mtime granularity, it is
        // a same-size replace that deliberately PRESERVES mtime -- exactly what
        // `cp -p`/`rsync -a`/`tar -p`/`touch -r` do. `(mtime, size)` alone would
        // miss this entirely, regardless of clock resolution. `ctime`/inode
        // close it: this test forces mtime back to its original value after a
        // same-size content rewrite and still expects the fingerprint to move.
        let dir = tmp("fingerprint-mtime-preserving");
        let f = dir.join("a.txt");
        std::fs::write(&f, b"aaaaa").unwrap();
        let meta_before = std::fs::metadata(&f).unwrap();
        let mtime_before = meta_before.modified().unwrap();
        let dur = mtime_before.duration_since(std::time::UNIX_EPOCH).unwrap();
        let before = stat_fingerprint(&dir).unwrap();

        // Same-size content change, then force mtime back to the original.
        std::fs::write(&f, b"bbbbb").unwrap();
        force_mtime(&f, dur.as_secs() as i64, dur.subsec_nanos() as i64);
        let mtime_after = std::fs::metadata(&f).unwrap().modified().unwrap();
        assert_eq!(
            mtime_after, mtime_before,
            "mtime must be forced back to its original value for this test to be meaningful"
        );

        let after = stat_fingerprint(&dir).unwrap();
        assert_ne!(
            before, after,
            "a same-size, mtime-preserving content change must still move the \
             fingerprint (ctime/inode must catch what mtime/size cannot)"
        );
    }

    #[test]
    fn hash_is_stable_for_same_content() {
        let dir = tmp("stable");
        std::fs::write(dir.join("a.txt"), b"hello").unwrap();
        let h1 = hash_path(&dir).unwrap();
        let h2 = hash_path(&dir).unwrap();
        assert_eq!(h1, h2);
    }

    #[test]
    fn hash_changes_when_content_changes() {
        let dir = tmp("change");
        let f = dir.join("a.txt");
        std::fs::write(&f, b"hello").unwrap();
        let before = hash_path(&dir).unwrap();
        std::fs::write(&f, b"hello!").unwrap();
        let after = hash_path(&dir).unwrap();
        assert_ne!(before, after);
    }

    #[test]
    fn single_file_and_dir_both_hash() {
        let dir = tmp("file");
        let f = dir.join("only.md");
        std::fs::write(&f, b"x").unwrap();
        assert!(!hash_path(&f).unwrap().is_empty());
        assert!(!hash_path(&dir).unwrap().is_empty());
    }

    /// A symlink that points to its own parent directory (a cycle) must not
    /// cause unbounded recursion or a stack overflow (LIFE-34).
    #[cfg(unix)]
    #[test]
    fn hash_path_symlink_cycle_does_not_overflow() {
        // spec: LIFE-34
        let dir = tmp("symlink-cycle");
        // Create a symlink inside the dir that points back to the dir itself.
        std::os::unix::fs::symlink(&*dir, dir.join("loop")).unwrap();
        // Before the fix this would infinite-recurse; after it must terminate.
        let result = hash_path(&dir);
        assert!(
            result.is_ok(),
            "symlink cycle must not overflow: {result:?}"
        );
    }

    /// A tree nested deeper than the LIFE-52 cap fails with a structured
    /// error instead of overflowing the stack.
    #[test]
    fn hash_path_depth_caps_a_pathological_tree() {
        // spec: LIFE-52
        let dir = tmp("deep-tree");
        let mut deep = dir.to_path_buf();
        for _ in 0..(crate::install::MAX_ITEM_TREE_DEPTH + 2) {
            deep.push("d");
        }
        std::fs::create_dir_all(&deep).unwrap();
        let result = hash_path(&dir);
        assert!(
            result.is_err(),
            "hash_path must refuse a tree deeper than the cap: {result:?}"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("nested directories"),
            "error message must name the depth cap: {msg}"
        );
        // The TUI-72 fingerprint walks the same tree, so it is capped too.
        let msg = stat_fingerprint(&dir)
            .expect_err("the fingerprint must refuse a tree deeper than the cap")
            .to_string();
        assert!(msg.contains("nested directories"), "{msg}");
    }

    /// A symlink's presence and its target must affect the hash so that adding,
    /// removing, or retargeting a symlink is detected as drift (LIFE-34).
    #[cfg(unix)]
    #[test]
    fn hash_path_symlink_target_affects_hash() {
        // spec: LIFE-34
        let dir = tmp("symlink-hash");
        std::fs::write(dir.join("file.txt"), b"content").unwrap();

        std::os::unix::fs::symlink("/target/a", dir.join("link")).unwrap();
        let h_a = hash_path(&dir).unwrap();

        // Retarget the symlink: hash must change.
        std::fs::remove_file(dir.join("link")).unwrap();
        std::os::unix::fs::symlink("/target/b", dir.join("link")).unwrap();
        let h_b = hash_path(&dir).unwrap();
        assert_ne!(h_a, h_b, "retargeting a symlink must change the hash");

        // Remove the symlink entirely: hash must change again.
        std::fs::remove_file(dir.join("link")).unwrap();
        let h_none = hash_path(&dir).unwrap();
        assert_ne!(h_a, h_none, "removing a symlink must change the hash");
    }

    /// Two entries `("ab", "c")` and `("a", "bc")` must hash differently.
    /// Without length-prefixed fields the old framing wrote `rel + '\0' +
    /// content` per entry, so the combined byte stream was identical for both.
    /// With length framing each field boundary is unambiguous.
    #[test]
    fn hash_length_framing_prevents_path_content_split_collision() {
        // spec: LIFE-35
        let dir_ab_c = tmp("framing-ab-c");
        std::fs::create_dir_all(dir_ab_c.join("ab")).unwrap();
        std::fs::write(dir_ab_c.join("ab").join("x"), b"c").unwrap();
        // Use a flat file named "ab" with content "c" in one dir...
        let dir1 = tmp("framing-flat-ab-c");
        std::fs::write(dir1.join("ab"), b"c").unwrap();

        // ...and a flat file named "a" with content "bc" in another dir.
        let dir2 = tmp("framing-flat-a-bc");
        std::fs::write(dir2.join("a"), b"bc").unwrap();

        let h1 = hash_path(&dir1).unwrap();
        let h2 = hash_path(&dir2).unwrap();
        assert_ne!(
            h1, h2,
            "entry ('ab','c') must not collide with ('a','bc') under length framing"
        );
    }

    /// A regular file whose name is the same as a symlink's target-rel path
    /// must hash differently from that symlink, so the two are not confused.
    /// The old framing used a `"symlink:"` key prefix that could be matched
    /// by a real file; the new framing uses a 1-byte type tag.
    #[cfg(unix)]
    #[test]
    fn hash_type_tag_prevents_file_symlink_collision() {
        // spec: LIFE-35
        // Dir 1: a regular file named "foo" with content = "/target".
        let dir1 = tmp("tag-file");
        std::fs::write(dir1.join("foo"), b"/target").unwrap();

        // Dir 2: a symlink named "foo" pointing to "/target".
        let dir2 = tmp("tag-symlink");
        std::os::unix::fs::symlink("/target", dir2.join("foo")).unwrap();

        let h1 = hash_path(&dir1).unwrap();
        let h2 = hash_path(&dir2).unwrap();
        assert_ne!(
            h1, h2,
            "a file and a symlink with matching name/content must not collide"
        );
    }

    /// A single symlink hashed directly via `hash_path` must not collide with
    /// a single regular file whose raw bytes equal the symlink's target string.
    #[cfg(unix)]
    #[test]
    fn hash_single_file_vs_single_symlink_distinct() {
        // spec: LIFE-35
        let dir = tmp("single-vs-sym");

        let file_path = dir.join("f");
        std::fs::write(&file_path, b"/target").unwrap();

        let sym_path = dir.join("s");
        std::os::unix::fs::symlink("/target", &sym_path).unwrap();

        let h_file = hash_path(&file_path).unwrap();
        let h_sym = hash_path(&sym_path).unwrap();
        assert_ne!(
            h_file, h_sym,
            "file with content '/target' must not collide with symlink -> '/target'"
        );
    }

    /// Two entries where swapping path/content bytes would look identical
    /// without length framing: `("abc", "")` vs `("ab", "c")`.
    #[test]
    fn hash_length_framing_empty_content_vs_suffix() {
        // spec: LIFE-35
        let dir1 = tmp("framing-abc-empty");
        std::fs::write(dir1.join("abc"), b"").unwrap();

        let dir2 = tmp("framing-ab-c2");
        std::fs::write(dir2.join("ab"), b"c").unwrap();

        let h1 = hash_path(&dir1).unwrap();
        let h2 = hash_path(&dir2).unwrap();
        assert_ne!(
            h1, h2,
            "entry ('abc','') must not collide with ('ab','c') under length framing"
        );
    }
}
