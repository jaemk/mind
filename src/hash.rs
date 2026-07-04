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
}

/// Hash an item path: a single file, or a directory hashed recursively.
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
pub fn hash_path(path: &Path) -> Result<String> {
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
        collect_files(path, path, &mut files)?;
        files.sort();
        // spec: LIFE-35 - length-prefixed fields prevent (path, content) split
        // collisions across entries.
        for (tag, rel, bytes) in files {
            h.write(&[tag]);
            h.write(&(rel.len() as u64).to_le_bytes());
            h.write(rel.as_bytes());
            h.write(&(bytes.len() as u64).to_le_bytes());
            h.write(&bytes);
        }
    } else {
        // Plain file: type tag + raw content. No length-prefix on content needed
        // for single-file hashes since there is only one field; the type tag
        // still distinguishes the file hash from any symlink hash.
        // spec: LIFE-35
        let bytes = std::fs::read(path).map_err(|e| MindError::io(path, e))?;
        h.write(b"F");
        h.write(&bytes);
    }
    Ok(h.finish_hex())
}

/// Walk `dir` and collect `(type_tag, relative_path_string, content_bytes)` triples.
///
/// Uses `symlink_metadata` at every step so symlinks are never followed
/// (LIFE-34). A symlink entry carries type tag `b'S'` and contributes its
/// link-target string as its content; a regular file carries `b'F'`. The
/// separate type tag prevents a file named `"symlink:foo"` from producing the
/// same triple as a symlink named `"foo"` (LIFE-35).
fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(u8, String, Vec<u8>)>) -> Result<()> {
    let rd = std::fs::read_dir(dir).map_err(|e| MindError::io(dir, e))?;
    for entry in rd {
        let entry = entry.map_err(|e| MindError::io(dir, e))?;
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path).map_err(|e| MindError::io(&path, e))?;
        let ft = meta.file_type();
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
                target.to_string_lossy().into_owned().into_bytes(),
            ));
        } else if ft.is_dir() {
            collect_files(root, &path, out)?;
        } else {
            // spec: LIFE-35
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            let bytes = std::fs::read(&path).map_err(|e| MindError::io(&path, e))?;
            out.push((b'F', rel, bytes));
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
