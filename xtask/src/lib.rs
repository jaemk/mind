//! Release tasks: the signing key pair and archive signing that `mind evolve`
//! verifies (spec/storage.md STO-88).

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::{self, Cursor, Write};
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};

/// A task failure: what failed, with its cause.
#[derive(Debug)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Parser)]
#[command(name = "xtask", about = "Release tasks for mind")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Write a new signing key pair (release.key) and its verifying key (release.pub).
    GenKey {
        /// Directory to write the keys into.
        #[arg(long)]
        out: PathBuf,
    },
    /// Sign release archives in place for `mind evolve` (STO-88).
    Sign {
        /// The 64-byte signing key pair `gen-key` wrote.
        #[arg(long)]
        key: PathBuf,
        /// The verifying key `mind evolve` embeds; `key` must be its pair.
        #[arg(long, default_value = "release.pub")]
        public: PathBuf,
        /// The `.tar.gz` archives to sign.
        #[arg(required = true)]
        archives: Vec<PathBuf>,
    },
}

/// Run the command line `args` (including the program name), printing to `out`.
pub fn run(args: impl IntoIterator<Item = OsString>, out: &mut impl Write) -> Result<()> {
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(e) if !e.use_stderr() => {
            write!(out, "{}", e.render())?;
            return Ok(());
        }
        Err(e) => return Err(Error(e.to_string())),
    };
    match cli.command {
        Command::GenKey { out: dir } => {
            let (private, public) = gen_key(&dir)?;
            writeln!(out, "{}\n{}", private.display(), public.display())?;
        }
        Command::Sign {
            key,
            public,
            archives,
        } => {
            let key = signing_key(&key, &public)?;
            for a in &archives {
                sign(&key, a)?;
                writeln!(out, "signed {}", a.display())?;
            }
        }
    }
    out.flush()?;
    Ok(())
}

/// Write a new key pair into `dir`: `release.key`, the 64-byte signing key pair
/// for the release workflow's `ZIPSIGN_KEY` secret, and `release.pub`, the
/// 32-byte verifying key `mind evolve` embeds. Refuses to overwrite either.
/// Returns the two paths.
pub fn gen_key(dir: &Path) -> Result<(PathBuf, PathBuf)> {
    let (private, public) = (dir.join("release.key"), dir.join("release.pub"));
    for p in [&private, &public] {
        if p.exists() {
            return Err(Error(format!("{} exists; not overwriting it", p.display())));
        }
    }
    // An ed25519 secret key is 32 random bytes.
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).map_err(|e| Error(format!("could not read random bytes: {e}")))?;
    let key = zipsign_api::SigningKey::from_bytes(&seed);
    fs::create_dir_all(dir).map_err(|e| context(dir, e))?;
    write_private_new(&private, &key.to_keypair_bytes())?;
    write_new(&public, key.verifying_key().as_bytes())?;
    Ok((private, public))
}

/// The signing key pair in `key`, checked against the verifying key in
/// `public`, so a key `mind evolve` would not accept signs nothing.
pub fn signing_key(key: &Path, public: &Path) -> Result<zipsign_api::SigningKey> {
    let file = fs::File::open(key).map_err(|e| context(key, e))?;
    let keys = zipsign_api::sign::read_signing_keys([Ok::<_, io::Error>(file)])
        .map_err(|e| Error(format!("{}: {e}", key.display())))?;
    let expected = fs::read(public).map_err(|e| context(public, e))?;
    let key = keys
        .into_iter()
        .next()
        .ok_or_else(|| Error("no signing key was read".to_string()))?;
    if key.verifying_key().as_bytes()[..] != expected[..] {
        return Err(Error(format!(
            "{}: not the key pair of the verifying key in {}",
            key_name(key.verifying_key().as_bytes()),
            public.display()
        )));
    }
    Ok(key)
}

/// A short, non-secret name for a verifying key.
fn key_name(public: &[u8]) -> String {
    let hex: String = public.iter().take(4).map(|b| format!("{b:02x}")).collect();
    format!("signing key {hex}...")
}

/// Sign the `.tar.gz` `archive` in place with `key`, as `self_update` verifies
/// it: over the archive's bytes, with its file name as the context.
pub fn sign(key: &zipsign_api::SigningKey, archive: &Path) -> Result<()> {
    let name = archive
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| n.ends_with(".tar.gz"))
        .ok_or_else(|| Error(format!("{}: not a .tar.gz file name", archive.display())))?;
    let bytes = fs::read(archive).map_err(|e| context(archive, e))?;
    let mut signed = Cursor::new(Vec::with_capacity(bytes.len() + 1024));
    zipsign_api::sign::copy_and_sign_tar(
        &mut Cursor::new(bytes),
        &mut signed,
        std::slice::from_ref(key),
        Some(name.as_bytes()),
    )
    .map_err(|e| Error(format!("{}: {e}", archive.display())))?;
    // Staged under a fresh random name beside the archive, so an existing file
    // or symlink there is never written through.
    let parent = archive
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(|e| context(archive, e))?;
    tmp.write_all(signed.get_ref())
        .map_err(|e| context(archive, e))?;
    let permissions = fs::metadata(archive)
        .map_err(|e| context(archive, e))?
        .permissions();
    tmp.as_file()
        .set_permissions(permissions)
        .map_err(|e| context(archive, e))?;
    tmp.persist(archive)
        .map(|_| ())
        .map_err(|e| context(archive, e.error))
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| context(path, e))?;
    f.write_all(bytes).map_err(|e| context(path, e))
}

fn write_private_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut f = options.open(path).map_err(|e| context(path, e))?;
    f.write_all(bytes).map_err(|e| context(path, e))
}

fn context(path: &Path, e: io::Error) -> Error {
    Error(format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gz(bytes: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(bytes).unwrap();
        e.finish().unwrap()
    }

    fn verifying_key(path: &Path) -> self_update::VerifyingKey {
        fs::read(path).unwrap().try_into().unwrap()
    }

    /// What `sign` writes passes the check `mind evolve` runs, under the
    /// archive's own name and only with the key pair's verifying key.
    #[test]
    fn signed_archives_verify() {
        let dir = tempfile::tempdir().unwrap();
        let (private, public_path) = gen_key(dir.path()).unwrap();
        let e = gen_key(dir.path()).unwrap_err();
        assert!(e.to_string().ends_with("exists; not overwriting it"), "{e}");
        let public = verifying_key(&public_path);
        let other_dir = tempfile::tempdir().unwrap();
        let (other_private, other_path) = gen_key(other_dir.path()).unwrap();
        let other = verifying_key(&other_path);
        // A key pair is checked against the verifying key it must match.
        let e = signing_key(&other_private, &public_path).unwrap_err();
        assert!(
            e.to_string()
                .contains("not the key pair of the verifying key"),
            "{e}"
        );
        let key = signing_key(&private, &public_path).unwrap();

        let archive = dir
            .path()
            .join("mind-1.0.0-x86_64-unknown-linux-musl.tar.gz");
        fs::write(&archive, gz(b"not really a tar")).unwrap();
        // Unsigned, it fails.
        assert!(self_update::verify_signature(&archive, &[public]).is_err());
        sign(&key, &archive).unwrap();
        self_update::verify_signature(&archive, &[public]).unwrap();
        assert!(self_update::verify_signature(&archive, &[other]).is_err());
        // The signature covers the name: the same bytes under another name fail.
        let renamed = dir
            .path()
            .join("mind-1.0.0-aarch64-unknown-linux-musl.tar.gz");
        fs::copy(&archive, &renamed).unwrap();
        assert!(self_update::verify_signature(&renamed, &[public]).is_err());
        // And the bytes.
        let mut bytes = fs::read(&archive).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        fs::write(&archive, &bytes).unwrap();
        assert!(self_update::verify_signature(&archive, &[public]).is_err());

        let e = sign(&key, &dir.path().join("a.zip")).unwrap_err();
        assert!(e.to_string().ends_with("not a .tar.gz file name"), "{e}");
    }

    #[cfg(unix)]
    #[test]
    fn gen_key_writes_a_private_key_only_the_owner_reads() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (private, public) = gen_key(dir.path()).unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&private), 0o600);
        assert_eq!(fs::read(&private).unwrap().len(), 64);
        assert_eq!(fs::read(&public).unwrap().len(), 32);
    }

    #[test]
    fn signing_key_names_a_missing_key_file() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.key");
        let e = signing_key(&missing, &dir.path().join("release.pub")).unwrap_err();
        assert!(e.to_string().contains("missing.key"), "{e}");
    }

    #[test]
    fn signing_keeps_the_archive_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("mind-1.0.0-test.tar.gz");
        fs::write(&archive, gz(b"archive contents")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&archive, fs::Permissions::from_mode(0o640)).unwrap();
        }
        let key = zipsign_api::SigningKey::from_bytes(&[7; 32]);
        sign(&key, &archive).unwrap();
        self_update::verify_signature(&archive, &[key.verifying_key().to_bytes()]).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&archive).unwrap().permissions().mode() & 0o777,
                0o640
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn signing_does_not_follow_a_symlink_beside_the_archive() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("mind-1.0.0-test.tar.gz");
        fs::write(&archive, gz(b"archive contents")).unwrap();
        let sentinel = dir.path().join("sentinel");
        fs::write(&sentinel, b"keep this target").unwrap();
        let staging = archive.with_file_name(".mind-1.0.0-test.tar.gz.signing");
        symlink(&sentinel, &staging).unwrap();
        let key = zipsign_api::SigningKey::from_bytes(&[7; 32]);
        sign(&key, &archive).unwrap();
        assert_eq!(fs::read(&sentinel).unwrap(), b"keep this target");
        assert!(fs::symlink_metadata(&staging).unwrap().is_symlink());
        assert!(fs::symlink_metadata(&archive).unwrap().is_file());
    }

    #[test]
    fn help_prints_to_out() {
        let mut out = Vec::new();
        run(["xtask".into(), "--help".into()], &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("gen-key") && text.contains("sign"), "{text}");
    }
}
