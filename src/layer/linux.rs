//! Linux backend: each volume is a btrfs subvolume. Bases are read-only
//! subvolumes and workspace repos are writable snapshots of them, created in
//! place at `~/.box/workspaces/<ws>/<repo>`, so nothing is ever mounted.
//!
//! Requirements: `~/.box/layers` and `~/.box/workspaces` on the same btrfs
//! filesystem and `btrfs-progs` installed. Snapshots work unprivileged.
//! Deleting a subvolume unprivileged needs the `user_subvol_rm_allowed` mount
//! option; without it, deletion falls back to removing the files one by one.

use anyhow::{bail, Context, Result};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::path_str;

/// Subvolumes are plain directories, so stores have no extension.
pub const STORE_EXT: Option<&str> = None;

/// Subvolumes are always accessible; activity is read from the checkout.
pub const DETACHABLE: bool = false;

/// A subvolume root contains nothing but the checkout.
pub const VOLUME_ROOT_EXCLUDES: &[&str] = &[];

/// Inode number of every btrfs subvolume root (`BTRFS_FIRST_FREE_OBJECTID`).
const SUBVOLUME_ROOT_INO: u64 = 256;

pub fn ensure_supported() -> Result<()> {
    Ok(())
}

/// Require that every path is on btrfs, all on the same filesystem (snapshots
/// cannot cross filesystems), and that `btrfs-progs` is installed.
pub fn preflight(paths: &[&Path]) -> Result<()> {
    let mounts = read_mounts()?;
    let mut first: Option<(&Path, &Mount)> = None;
    for path in paths {
        let canonical = fs::canonicalize(path)
            .with_context(|| format!("failed to resolve {}", path.display()))?;
        let mount = containing_mount(&mounts, &canonical)
            .ok_or_else(|| anyhow::anyhow!("no mount found for {}", canonical.display()))?;
        if mount.fstype != "btrfs" {
            bail!(
                "The 'layer' strategy on Linux requires btrfs, but {} is on {} ({}). \
                 Put the box root (~/.box or $BOX_ROOT) on a btrfs filesystem.",
                path.display(),
                mount.fstype,
                mount.mount_point.display()
            );
        }
        match first {
            None => first = Some((path, mount)),
            Some((first_path, first_mount)) if first_mount.source != mount.source => bail!(
                "The 'layer' strategy on Linux requires {} and {} on the same btrfs filesystem.",
                first_path.display(),
                path.display()
            ),
            Some(_) => {}
        }
    }
    btrfs(&["--version"]).context("btrfs-progs is required for the 'layer' strategy")?;
    Ok(())
}

/// Where a workspace repo's backing store lives: the checkout itself.
pub fn ws_store(ws_root: &Path, repo_name: &str) -> PathBuf {
    ws_root.join(repo_name)
}

/// A subvolume is used in place.
pub fn staging_mount_point(store: &Path) -> PathBuf {
    store.to_path_buf()
}

/// Run `btrfs` and fail on non-zero exit. Unprivileged runs print harmless
/// warnings (e.g. "cannot read default subvolume id"), so output is captured
/// and only reported on failure.
fn btrfs(args: &[&str]) -> Result<()> {
    let output = Command::new("btrfs")
        .args(args)
        .output()
        .context("failed to run btrfs")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let message: Vec<&str> = stderr
            .lines()
            .filter(|l| !l.starts_with("WARNING:"))
            .collect();
        bail!(
            "btrfs {} failed: {}",
            args.iter().take(2).copied().collect::<Vec<_>>().join(" "),
            message.join("\n").trim()
        );
    }
    Ok(())
}

/// Create an empty subvolume at `store`.
pub fn create_store(store: &Path, _volname: &str) -> Result<()> {
    btrfs(&["subvolume", "create", path_str(store)?])
}

/// Writable snapshot of `src` at `dst` (which must not exist: `btrfs` would
/// otherwise create the snapshot inside it).
pub fn clone_store(src: &Path, dst: &Path) -> Result<()> {
    if fs::symlink_metadata(dst).is_ok() {
        bail!("{} already exists", dst.display());
    }
    btrfs(&["subvolume", "snapshot", path_str(src)?, path_str(dst)?]).with_context(|| {
        format!(
            "failed to snapshot {} to {} (both must be on the same btrfs filesystem)",
            src.display(),
            dst.display()
        )
    })
}

/// Subvolumes need no attaching.
pub fn open(store: &Path, mount_point: &Path) -> Result<()> {
    debug_assert_eq!(store, mount_point);
    if !is_subvolume(store) {
        bail!("{} is not a btrfs subvolume", store.display());
    }
    Ok(())
}

/// Subvolumes need no detaching.
pub fn close(_mount_point: &Path) -> Result<()> {
    Ok(())
}

/// Make a published base read-only so it can't be modified by accident.
pub fn seal(store: &Path) -> Result<()> {
    btrfs(&["property", "set", "-ts", path_str(store)?, "ro", "true"])
}

fn is_subvolume(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_dir() && m.ino() == SUBVOLUME_ROOT_INO)
}

/// Delete a subvolume (or plain directory). Missing stores are fine.
pub fn delete_store(store: &Path) -> Result<()> {
    if fs::symlink_metadata(store).is_err() {
        return Ok(());
    }
    if is_subvolume(store) {
        // Read-only subvolumes can be neither deleted nor emptied unprivileged.
        let _ = btrfs(&["property", "set", "-ts", path_str(store)?, "ro", "false"]);
        // Needs root or the `user_subvol_rm_allowed` mount option.
        if btrfs(&["subvolume", "delete", path_str(store)?]).is_ok()
            && fs::symlink_metadata(store).is_err()
        {
            return Ok(());
        }
    }
    // Fallback: the owner may remove the files and then rmdir the empty
    // subvolume root.
    super::remove_dir_all_if_exists(store)
}

/// Nothing to prepare in a fresh subvolume.
pub fn prepare_volume(_mount_point: &Path) -> Result<()> {
    Ok(())
}

/// Delete `dir` and everything in it, including subvolumes among its children.
pub fn purge_dir(dir: &Path) -> Result<()> {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                delete_store(&entry.path())?;
            }
        }
    }
    super::remove_dir_all_if_exists(dir)
}

/// True if a filesystem is mounted at `path`. Subvolumes have their own
/// device numbers, so comparing `st_dev` with the parent is not enough here.
pub fn is_mount_point(path: &Path) -> bool {
    let Ok(canonical) = fs::canonicalize(path) else {
        return false;
    };
    read_mounts()
        .map(|mounts| mounts.iter().any(|m| m.mount_point == canonical))
        .unwrap_or(false)
}

// -- /proc/self/mountinfo --

#[derive(Debug, PartialEq)]
struct Mount {
    mount_point: PathBuf,
    fstype: String,
    source: String,
}

fn read_mounts() -> Result<Vec<Mount>> {
    let text = fs::read_to_string("/proc/self/mountinfo")
        .context("failed to read /proc/self/mountinfo")?;
    Ok(parse_mountinfo(&text))
}

/// Parse mountinfo lines:
/// `id parent major:minor root mount_point options [optional...] - fstype source super_options`
fn parse_mountinfo(text: &str) -> Vec<Mount> {
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(' ').collect();
            let sep = fields.iter().position(|f| *f == "-")?;
            if sep < 6 {
                return None;
            }
            Some(Mount {
                mount_point: PathBuf::from(unescape(fields[4])),
                fstype: fields.get(sep + 1)?.to_string(),
                source: unescape(fields.get(sep + 2)?),
            })
        })
        .collect()
}

/// Undo the kernel's octal escaping of space, tab, newline, and backslash.
fn unescape(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 4 <= bytes.len() {
            let oct = &bytes[i + 1..i + 4];
            if oct.iter().all(|b| (b'0'..=b'7').contains(b)) {
                let value = oct
                    .iter()
                    .fold(0u32, |acc, b| acc * 8 + u32::from(b - b'0'));
                if let Ok(byte) = u8::try_from(value) {
                    out.push(byte);
                    i += 4;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The mount containing `path` (canonical): the longest matching mount point,
/// the latest one when several are stacked on the same path.
fn containing_mount<'a>(mounts: &'a [Mount], path: &Path) -> Option<&'a Mount> {
    let mut best: Option<&Mount> = None;
    for mount in mounts {
        if path.starts_with(&mount.mount_point)
            && best.is_none_or(|b| {
                mount.mount_point.components().count() >= b.mount_point.components().count()
            })
        {
            best = Some(mount);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
22 1 8:1 / / rw,relatime shared:1 - ext4 /dev/root rw
40 22 0:35 / /home/u/.box rw,noatime shared:20 - btrfs /dev/sdb rw,user_subvol_rm_allowed
41 22 0:36 /sub /mnt/with\\040space rw - btrfs /dev/sdc rw
42 40 0:37 / /home/u/.box rw - btrfs /dev/sdd rw
";

    #[test]
    fn test_parse_mountinfo() {
        let mounts = parse_mountinfo(SAMPLE);
        assert_eq!(mounts.len(), 4);
        assert_eq!(
            mounts[1],
            Mount {
                mount_point: PathBuf::from("/home/u/.box"),
                fstype: "btrfs".into(),
                source: "/dev/sdb".into(),
            }
        );
        assert_eq!(mounts[2].mount_point, PathBuf::from("/mnt/with space"));
    }

    #[test]
    fn test_unescape() {
        assert_eq!(unescape("a\\040b\\011c\\134d"), "a b\tc\\d");
        assert_eq!(unescape("plain"), "plain");
        assert_eq!(unescape("trailing\\04"), "trailing\\04");
        assert_eq!(unescape("bad\\089"), "bad\\089");
    }

    #[test]
    fn test_containing_mount() {
        let mounts = parse_mountinfo(SAMPLE);
        let m = containing_mount(&mounts, Path::new("/home/u/.box/layers/x")).unwrap();
        // The later mount stacked on the same path wins.
        assert_eq!(m.source, "/dev/sdd");
        let m = containing_mount(&mounts, Path::new("/home/u/.boxer")).unwrap();
        assert_eq!(m.fstype, "ext4");
        let m = containing_mount(&mounts, Path::new("/mnt/with space/a")).unwrap();
        assert_eq!(m.source, "/dev/sdc");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_is_mount_point() {
        let tmp = tempfile::tempdir().unwrap();
        let child = tmp.path().join("child");
        fs::create_dir(&child).unwrap();
        assert!(!is_mount_point(&child));
        assert!(!is_mount_point(&tmp.path().join("missing")));
        assert!(is_mount_point(Path::new("/")));
        assert!(is_mount_point(Path::new("/proc")));
    }
}
