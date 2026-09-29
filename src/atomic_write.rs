//! Replace a file's bytes in one rename, so readers never see a partial write.

use std::io::{self, Write};
use std::path::Path;

/// Writes `bytes` to a temp file beside `canonical`, copies its mode bits, syncs, then
/// renames it over `canonical`. Pass the canonical path so a symlink is kept, not replaced.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "callers need the checked variant for now")
)]
pub(crate) fn write_atomic(canonical: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic_if(canonical, bytes, || true).map(|_| ())
}

/// [`write_atomic`], asking `still_ok` just before the rename. Returns `false`, with the
/// target untouched and the temp file removed, when it declines.
pub(crate) fn write_atomic_if(
    canonical: &Path,
    bytes: &[u8],
    still_ok: impl FnOnce() -> bool,
) -> io::Result<bool> {
    let dir = canonical
        .parent()
        .ok_or_else(|| io::Error::other("target has no parent directory"))?;
    // Same directory, so the rename never crosses filesystems.
    let mut tmp = tempfile::Builder::new()
        .prefix(".birta-write-")
        .tempfile_in(dir)?;
    tmp.write_all(bytes)?;
    if let Ok(meta) = std::fs::metadata(canonical) {
        tmp.as_file().set_permissions(meta.permissions())?;
    }
    tmp.as_file().sync_data()?;
    if !still_ok() {
        return Ok(false);
    }
    tmp.persist(canonical).map_err(|e| e.error)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    use super::*;

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn replaces_bytes_exactly() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("doc.md");
        std::fs::write(&file, "old\n").unwrap();

        let bytes = b"\xef\xbb\xbfnew\r\nno final newline";
        write_atomic(&file, bytes).unwrap();

        assert_eq!(std::fs::read(&file).unwrap(), bytes);
    }

    #[test]
    fn replaces_by_rename() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("doc.md");
        std::fs::write(&file, "old\n").unwrap();
        let before = std::fs::metadata(&file).unwrap().ino();

        write_atomic(&file, b"new\n").unwrap();

        // An in-place write keeps the inode; a rename from a temp file does not.
        assert_ne!(std::fs::metadata(&file).unwrap().ino(), before);
    }

    #[test]
    fn keeps_mode_bits() {
        let dir = tempfile::tempdir().unwrap();
        for mode in [0o600, 0o640, 0o755] {
            let file = dir.path().join(format!("doc-{mode:o}.md"));
            std::fs::write(&file, "old\n").unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();

            write_atomic(&file, b"new\n").unwrap();

            let got = std::fs::metadata(&file).unwrap().permissions().mode() & 0o7777;
            assert_eq!(got, mode, "mode {mode:o}");
        }
    }

    #[test]
    fn leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("doc.md");
        std::fs::write(&file, "old\n").unwrap();

        write_atomic(&file, b"new\n").unwrap();

        assert_eq!(entries(dir.path()), ["doc.md"]);
    }

    #[test]
    fn failed_write_leaves_target_and_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let file = sub.join("doc.md");
        std::fs::write(&file, "old\n").unwrap();
        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o555)).unwrap();

        let result = write_atomic(&file, b"new\n");
        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert!(result.is_err(), "a read-only folder must fail the write");
        assert_eq!(std::fs::read(&file).unwrap(), b"old\n");
        assert_eq!(entries(&sub), ["doc.md"]);
    }

    const CHILD_TARGET: &str = "BIRTA_ATOMIC_CHILD_TARGET";

    #[test]
    #[ignore = "run by temp_file_lives_beside_the_target in a child process"]
    fn child_write() {
        let target = std::env::var_os(CHILD_TARGET).expect("run by the parent test");
        write_atomic(Path::new(&target), b"new\n").unwrap();
    }

    #[test]
    fn temp_file_lives_beside_the_target() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("doc.md");
        std::fs::write(&file, "old\n").unwrap();
        let tmpdir = dir.path().join("no-temp-here");
        std::fs::create_dir(&tmpdir).unwrap();
        std::fs::set_permissions(&tmpdir, std::fs::Permissions::from_mode(0o555)).unwrap();

        // A temp file in $TMPDIR would fail here, since that folder is read-only.
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "atomic_write::tests::child_write", "--ignored"])
            .env("TMPDIR", &tmpdir)
            .env(CHILD_TARGET, &file)
            .output()
            .unwrap();
        std::fs::set_permissions(&tmpdir, std::fs::Permissions::from_mode(0o755)).unwrap();

        let stdout = String::from_utf8_lossy(&child.stdout);
        assert!(child.status.success(), "child write failed: {stdout}");
        assert_eq!(std::fs::read(&file).unwrap(), b"new\n");
    }
}
