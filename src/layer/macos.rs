//! macOS backend: each volume is an APFS sparse disk image attached with
//! `hdiutil`. Images are cloned with `clonefile` (via `fs::copy`), which is
//! copy-on-write on APFS.

use anyhow::{bail, Context, Result};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::path_str;

/// Maximum size of a sparse image. Space is only allocated on write.
const IMAGE_SIZE: &str = "512g";

/// Extension of a volume's backing store.
pub const STORE_EXT: Option<&str> = Some("sparseimage");

/// Workspace volumes can be detached (e.g. by a reboot), so their contents
/// may be unavailable; activity is judged from the image file instead.
pub const DETACHABLE: bool = true;

/// Files macOS may create at the root of a mounted APFS volume. A layer
/// worktree's root is the volume root, so hide them from `git status`.
pub const VOLUME_ROOT_EXCLUDES: &[&str] = &[
    "/.fseventsd/",
    "/.metadata_never_index",
    "/.Spotlight-V100/",
    "/.Trashes/",
    "/.TemporaryItems/",
    "/.DocumentRevisions-V100/",
];

pub fn ensure_supported() -> Result<()> {
    if cfg!(target_os = "macos") {
        Ok(())
    } else {
        bail!("The 'layer' strategy is only supported on macOS and Linux (btrfs).")
    }
}

/// No filesystem requirements beyond APFS, which macOS always has.
pub fn preflight(_paths: &[&Path]) -> Result<()> {
    Ok(())
}

/// Where a workspace repo's backing store lives.
pub fn ws_store(ws_root: &Path, repo_name: &str) -> PathBuf {
    ws_root
        .join(super::IMAGES_DIR)
        .join(format!("{}.{}", repo_name, STORE_EXT.unwrap()))
}

/// Where a staging store is attached while a base is built.
pub fn staging_mount_point(store: &Path) -> PathBuf {
    store.with_extension("mnt")
}

fn hdiutil(args: &[&str]) -> Result<()> {
    let output = Command::new("hdiutil")
        .args(args)
        .output()
        .context("failed to run hdiutil")?;
    if !output.status.success() {
        bail!(
            "hdiutil {} failed: {}",
            args.first().copied().unwrap_or_default(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Create an empty volume store at `store`.
pub fn create_store(store: &Path, volname: &str) -> Result<()> {
    // hdiutil appends the `.sparseimage` extension itself.
    hdiutil(&[
        "create",
        "-quiet",
        "-size",
        IMAGE_SIZE,
        "-type",
        "SPARSE",
        "-fs",
        "APFS",
        "-volname",
        volname,
        path_str(&store.with_extension(""))?,
    ])
}

/// Copy-on-write clone `src` to `dst` (which must not exist).
pub fn clone_store(src: &Path, dst: &Path) -> Result<()> {
    if dst.exists() {
        bail!("{} already exists", dst.display());
    }
    fs::copy(src, dst)
        .with_context(|| format!("failed to clone {} to {}", src.display(), dst.display()))?;
    Ok(())
}

/// Make the volume in `store` available at `mount_point`.
pub fn open(store: &Path, mount_point: &Path) -> Result<()> {
    fs::create_dir_all(mount_point)?;
    hdiutil(&[
        "attach",
        "-quiet",
        "-nobrowse",
        "-noautoopen",
        "-noverify",
        "-mountpoint",
        path_str(mount_point)?,
        path_str(store)?,
    ])
}

/// Detach the volume mounted at `mount_point` (forcefully if a normal detach
/// fails). Succeeds when nothing is mounted there afterwards.
pub fn close(mount_point: &Path) -> Result<()> {
    if !is_mount_point(mount_point) {
        return Ok(());
    }
    let mp = path_str(mount_point)?;
    if hdiutil(&["detach", "-quiet", mp]).is_err() {
        let _ = hdiutil(&["detach", "-quiet", "-force", mp]);
    }
    if is_mount_point(mount_point) {
        bail!("failed to detach {}", mount_point.display());
    }
    Ok(())
}

/// Mark a finished base store immutable. Base images are never attached
/// read-write after publishing, so nothing to do.
pub fn seal(_store: &Path) -> Result<()> {
    Ok(())
}

/// Delete a (detached) store. Missing stores are fine.
pub fn delete_store(store: &Path) -> Result<()> {
    super::remove_file_if_exists(store)
}

/// Prepare a freshly created volume before it is populated.
pub fn prepare_volume(mount_point: &Path) -> Result<()> {
    fs::write(mount_point.join(".metadata_never_index"), "")?;
    Ok(())
}

/// Delete `dir` and everything in it, detaching any mounted children first.
pub fn purge_dir(dir: &Path) -> Result<()> {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            close(&entry.path())?;
        }
    }
    super::remove_dir_all_if_exists(dir)
}

/// True if `path` is a directory on a different device than its parent.
pub fn is_mount_point(path: &Path) -> bool {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    if !meta.is_dir() {
        return false;
    }
    let Some(parent) = path.parent() else {
        return false;
    };
    match fs::metadata(parent) {
        Ok(parent_meta) => meta.dev() != parent_meta.dev(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_mount_point() {
        let tmp = tempfile::tempdir().unwrap();
        let child = tmp.path().join("child");
        fs::create_dir(&child).unwrap();
        assert!(!is_mount_point(&child));
        assert!(!is_mount_point(&tmp.path().join("missing")));
        assert!(is_mount_point(Path::new("/dev")));
    }
}
