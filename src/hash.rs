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
pub fn hash_path(path: &Path) -> Result<String> {
    let mut h = Fnv::new();
    let meta = std::fs::symlink_metadata(path).map_err(|e| MindError::io(path, e))?;
    if meta.file_type().is_symlink() {
        // Hash the link target string so a retarget changes the hash, and so
        // no external content is read through the link.
        let target = std::fs::read_link(path).map_err(|e| MindError::io(path, e))?;
        h.write(b"symlink\0");
        h.write(target.to_string_lossy().as_bytes());
    } else if meta.is_dir() {
        let mut files = Vec::new();
        collect_files(path, path, &mut files)?;
        files.sort();
        for (rel, bytes) in files {
            h.write(rel.as_bytes());
            h.write(b"\0");
            h.write(&bytes);
        }
    } else {
        let bytes = std::fs::read(path).map_err(|e| MindError::io(path, e))?;
        h.write(&bytes);
    }
    Ok(h.finish_hex())
}

/// Walk `dir` and collect `(relative_path_string, content_bytes)` pairs.
///
/// Uses `symlink_metadata` at every step so symlinks are never followed
/// (LIFE-34). A symlink entry contributes its link-target string as its
/// "content" (prefixed with `"symlink:"` in the relative path key), so a
/// retargeting changes the hash and a cyclic symlink cannot recurse.
fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) -> Result<()> {
    let rd = std::fs::read_dir(dir).map_err(|e| MindError::io(dir, e))?;
    for entry in rd {
        let entry = entry.map_err(|e| MindError::io(dir, e))?;
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path).map_err(|e| MindError::io(&path, e))?;
        let ft = meta.file_type();
        if ft.is_symlink() {
            let target = std::fs::read_link(&path).map_err(|e| MindError::io(&path, e))?;
            let rel = format!(
                "symlink:{}",
                path.strip_prefix(root).unwrap_or(&path).to_string_lossy()
            );
            out.push((rel, target.to_string_lossy().into_owned().into_bytes()));
        } else if ft.is_dir() {
            collect_files(root, &path, out)?;
        } else {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            let bytes = std::fs::read(&path).map_err(|e| MindError::io(&path, e))?;
            out.push((rel, bytes));
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
}
