//! `layer` workspace strategy (macOS).
//!
//! Each repo in a workspace is its own APFS sparse disk image mounted at
//! `~/.box/workspaces/<ws>/<repo>`. The image is created by cloning
//! (`clonefile`, copy-on-write) a per-repo *base* image that already contains
//! a checkout, so creating a workspace costs a metadata-only file clone plus an
//! attach instead of writing every tracked file.
//!
//! Layout:
//! - `~/.box/layers/<repo>/bases/<commit>.sparseimage` + `<commit>.index`:
//!   published bases. Never modified after publishing.
//! - `~/.box/layers/<repo>/current`: commit of the newest published base.
//! - `~/.box/layers/<repo>/lock`: shared while cloning a base, exclusive while
//!   publishing or garbage-collecting bases.
//! - `~/.box/layers/<repo>/build.lock`: serializes base builds.
//! - `~/.box/layers/<repo>/staging/`: scratch space for base builds.
//! - `~/.box/workspaces/<ws>/.box-images/<repo>.sparseimage`: workspace image.
//! - `<bare>/worktrees/box-layer-ws-<ws>`: the workspace's git admin dir. It
//!   is locked so `git worktree prune` keeps it while the image is detached.

use anyhow::{bail, Context, Result};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::config;
use crate::repo::RepoEntry;

/// Maximum size of a sparse image. Space is only allocated on write.
const IMAGE_SIZE: &str = "512g";
const IMAGE_EXT: &str = "sparseimage";
/// Hidden directory inside a workspace root holding the workspace images.
pub const IMAGES_DIR: &str = ".box-images";
const WS_ADMIN_PREFIX: &str = "box-layer-ws-";
const BUILD_ADMIN_PREFIX: &str = "box-layer-build-";
const LOCK_REASON: &str = "box layer workspace (disk image may be detached)";

const EXCLUDE_BEGIN: &str = "# >>> box layer volume root >>>";
const EXCLUDE_END: &str = "# <<< box layer volume root <<<";
/// Files macOS may create at the root of a mounted APFS volume. A layer
/// worktree's root is the volume root, so hide them from `git status`.
const VOLUME_ROOT_EXCLUDES: &[&str] = &[
    "/.fseventsd/",
    "/.metadata_never_index",
    "/.Spotlight-V100/",
    "/.Trashes/",
    "/.TemporaryItems/",
    "/.DocumentRevisions-V100/",
];

/// Environment variable controlling base refreshes after workspace creation:
/// `background` (default), `sync`, or `off`.
const REFRESH_ENV: &str = "BOX_LAYER_REFRESH";

pub fn ensure_supported() -> Result<()> {
    if cfg!(target_os = "macos") {
        Ok(())
    } else {
        bail!("The 'layer' strategy is only supported on macOS.")
    }
}

// -- Paths --

fn layers_dir(repo_name: &str) -> Result<PathBuf> {
    Ok(config::box_root()?.join("layers").join(repo_name))
}

pub fn image_path(ws_root: &Path, repo_name: &str) -> PathBuf {
    ws_root
        .join(IMAGES_DIR)
        .join(format!("{}.{}", repo_name, IMAGE_EXT))
}

pub fn ws_admin_dir(bare: &str, session: &str) -> PathBuf {
    Path::new(bare)
        .join("worktrees")
        .join(format!("{}{}", WS_ADMIN_PREFIX, session))
}

// -- Small helpers --

/// Run a git command, append its output to `log`, and fail on non-zero exit.
fn git_logged(args: &[&str], log: &mut String) -> Result<()> {
    let output = Command::new("git")
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .context("failed to run git")?;
    log.push_str(&String::from_utf8_lossy(&output.stdout));
    log.push_str(&String::from_utf8_lossy(&output.stderr));
    if !output.status.success() {
        bail!("git {} failed", args.join(" "));
    }
    Ok(())
}

/// Run a git command and return its trimmed stdout.
fn git_stdout(args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .context("failed to run git")?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
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

fn path_str(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| anyhow::anyhow!("non-UTF-8 path: {}", path.display()))
}

fn remove_file_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("failed to remove {}", path.display())),
    }
}

fn remove_dir_all_if_exists(path: &Path) -> Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("failed to remove {}", path.display())),
    }
}

fn open_lock(path: &Path) -> Result<fs::File> {
    fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("failed to open lock {}", path.display()))
}

fn unique_tag() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("{}-{}", std::process::id(), nanos)
}

// -- Mounts --

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

fn attach(image: &Path, mount_point: &Path) -> Result<()> {
    fs::create_dir_all(mount_point)?;
    hdiutil(&[
        "attach",
        "-quiet",
        "-nobrowse",
        "-noautoopen",
        "-noverify",
        "-mountpoint",
        path_str(mount_point)?,
        path_str(image)?,
    ])
}

/// Detach the volume mounted at `mount_point` (forcefully if a normal detach
/// fails). Succeeds when nothing is mounted there afterwards.
pub fn detach(mount_point: &Path) -> Result<()> {
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

// -- git admin entries --

/// Make git ignore files macOS creates at a volume root. Idempotent.
pub fn ensure_volume_excludes(bare: &str) -> Result<()> {
    let path = Path::new(bare).join("info").join("exclude");
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let updated = with_volume_excludes(&existing);
    if updated != existing {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, updated)?;
    }
    Ok(())
}

fn with_volume_excludes(existing: &str) -> String {
    if existing.contains(EXCLUDE_BEGIN) {
        return existing.to_string();
    }
    let mut out = existing.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(EXCLUDE_BEGIN);
    out.push('\n');
    for pattern in VOLUME_ROOT_EXCLUDES {
        out.push_str(pattern);
        out.push('\n');
    }
    out.push_str(EXCLUDE_END);
    out.push('\n');
    out
}

/// Register `mount_point` as a linked worktree of `bare` by writing the admin
/// dir and the worktree's `.git` file directly. `git worktree add` refuses a
/// non-empty directory, and the volume root is never empty.
fn write_admin(
    admin: &Path,
    mount_point: &Path,
    head: &str,
    index: Option<&Path>,
    locked: bool,
) -> Result<()> {
    fs::create_dir_all(admin)?;
    fs::write(
        admin.join("gitdir"),
        format!("{}\n", mount_point.join(".git").display()),
    )?;
    fs::write(admin.join("commondir"), "../..\n")?;
    fs::write(admin.join("HEAD"), format!("{}\n", head))?;
    if let Some(index) = index {
        fs::copy(index, admin.join("index"))?;
    }
    if locked {
        fs::write(admin.join("locked"), LOCK_REASON)?;
    }
    fs::write(
        mount_point.join(".git"),
        format!("gitdir: {}\n", admin.display()),
    )?;
    Ok(())
}

fn zero_oid(dir: &str) -> String {
    let format = git_stdout(&["-C", dir, "rev-parse", "--show-object-format"])
        .unwrap_or_else(|_| "sha1".to_string());
    let len = if format == "sha256" { 64 } else { 40 };
    "0".repeat(len)
}

fn checked_out_elsewhere(bare: &str, branch: &str) -> bool {
    let wanted = format!("branch refs/heads/{}", branch);
    git_stdout(&["-C", bare, "worktree", "list", "--porcelain"])
        .map(|out| out.lines().any(|l| l == wanted))
        .unwrap_or(false)
}

// -- Bases --

struct Base {
    commit: String,
    image: PathBuf,
    index: PathBuf,
}

fn current_base(dir: &Path) -> Option<Base> {
    let commit = fs::read_to_string(dir.join("current")).ok()?;
    let commit = commit.trim().to_string();
    if commit.is_empty() {
        return None;
    }
    let bases = dir.join("bases");
    let image = bases.join(format!("{}.{}", commit, IMAGE_EXT));
    let index = bases.join(format!("{}.index", commit));
    if image.is_file() && index.is_file() {
        Some(Base {
            commit,
            image,
            index,
        })
    } else {
        None
    }
}

/// Remove leftovers of interrupted builds. Caller must hold `build.lock`.
fn clean_staging(staging: &Path, bare: &str) -> Result<()> {
    if let Ok(entries) = fs::read_dir(staging) {
        for entry in entries.flatten() {
            let path = entry.path();
            if is_mount_point(&path) {
                detach(&path)?;
            }
        }
    }
    remove_dir_all_if_exists(staging)?;
    if let Ok(entries) = fs::read_dir(Path::new(bare).join("worktrees")) {
        for entry in entries.flatten() {
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with(BUILD_ADMIN_PREFIX)
            {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
    }
    Ok(())
}

/// Build and publish a base image for `target`, rolling forward from the
/// current base when there is one. Caller must hold `build.lock`.
fn build_base(repo: &RepoEntry, target: &str, log: &mut String) -> Result<()> {
    let dir = layers_dir(&repo.name)?;
    let staging = dir.join("staging");
    clean_staging(&staging, &repo.path)?;
    fs::create_dir_all(&staging)?;
    fs::create_dir_all(dir.join("bases"))?;

    let tag = unique_tag();
    let image = staging.join(format!("{}.{}", tag, IMAGE_EXT));
    let staged_index = staging.join(format!("{}.index", tag));
    let mount_point = staging.join(format!("{}.mnt", tag));

    let from = {
        let lock = open_lock(&dir.join("lock"))?;
        lock.lock_shared()?;
        match current_base(&dir) {
            Some(base) => {
                fs::copy(&base.image, &image)?;
                fs::copy(&base.index, &staged_index)?;
                Some(base.commit)
            }
            None => None,
        }
    };
    if from.is_none() {
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
            &repo.name,
            path_str(&staging.join(&tag))?,
        ])?;
    }

    attach(&image, &mount_point)?;
    let admin = Path::new(&repo.path)
        .join("worktrees")
        .join(format!("{}{}", BUILD_ADMIN_PREFIX, tag));
    let populated = (|| -> Result<()> {
        fs::write(mount_point.join(".metadata_never_index"), "")?;
        let head = from.as_deref().unwrap_or(target);
        let index = from.as_ref().map(|_| staged_index.as_path());
        write_admin(&admin, &mount_point, head, index, false)?;
        let mp = path_str(&mount_point)?;
        git_logged(
            &[
                "-C",
                mp,
                "-c",
                "core.hooksPath=/dev/null",
                "reset",
                "-q",
                "--hard",
                target,
            ],
            log,
        )?;
        // Let the checkout's mtimes fall strictly behind the index mtime so
        // the published index has no racily-clean entries to re-hash later.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let _ = git_logged(&["-C", mp, "update-index", "-q", "--refresh"], log);
        fs::copy(admin.join("index"), &staged_index)?;
        Ok(())
    })();
    let _ = fs::remove_dir_all(&admin);
    let detached = detach(&mount_point);
    populated?;
    detached?;
    let _ = fs::remove_dir(&mount_point);

    // Publish: index first, then image, then the `current` pointer.
    let lock = open_lock(&dir.join("lock"))?;
    lock.lock()?;
    let bases = dir.join("bases");
    fs::rename(&staged_index, bases.join(format!("{}.index", target)))?;
    fs::rename(&image, bases.join(format!("{}.{}", target, IMAGE_EXT)))?;
    let tmp_current = dir.join(format!("current.{}", tag));
    fs::write(&tmp_current, format!("{}\n", target))?;
    fs::rename(&tmp_current, dir.join("current"))?;
    // Workspace images are independent clones, so older bases can go.
    if let Ok(entries) = fs::read_dir(&bases) {
        let keep = format!("{}.", target);
        for entry in entries.flatten() {
            if !entry.file_name().to_string_lossy().starts_with(&keep) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    Ok(())
}

/// Return once a base exists for the repo, building one at `target` if none.
fn ensure_base(repo: &RepoEntry, target: &str, log: &mut String) -> Result<()> {
    let dir = layers_dir(&repo.name)?;
    fs::create_dir_all(&dir)?;
    if current_base(&dir).is_some() {
        return Ok(());
    }
    let build = open_lock(&dir.join("build.lock"))?;
    build.lock()?;
    if current_base(&dir).is_some() {
        return Ok(());
    }
    log.push_str(&format!("building base image for {}\n", repo.name));
    build_base(repo, target, log)
}

/// Roll the repo's base forward to the bare repo's current HEAD. Skips when
/// another build is running or the base is already current.
pub fn refresh_base(repo: &RepoEntry) -> Result<()> {
    ensure_supported()?;
    let dir = layers_dir(&repo.name)?;
    fs::create_dir_all(&dir)?;
    let build = open_lock(&dir.join("build.lock"))?;
    match build.try_lock() {
        Ok(()) => {}
        Err(fs::TryLockError::WouldBlock) => return Ok(()),
        Err(fs::TryLockError::Error(e)) => return Err(e.into()),
    }
    let target = git_stdout(&["-C", &repo.path, "rev-parse", "--verify", "HEAD^{commit}"])?;
    if current_base(&dir).is_some_and(|b| b.commit == target) {
        return Ok(());
    }
    let mut log = String::new();
    let result = build_base(repo, &target, &mut log);
    if !log.is_empty() {
        eprint!("{}", log);
    }
    result
}

fn schedule_refresh(repo: &RepoEntry) {
    match std::env::var(REFRESH_ENV).as_deref() {
        Ok("off") => {}
        Ok("sync") => {
            let _ = refresh_base(repo);
        }
        _ => spawn_background_refresh(repo),
    }
}

fn spawn_background_refresh(repo: &RepoEntry) {
    // Unit tests run inside the test harness binary; never re-exec it.
    if cfg!(test) {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Ok(dir) = layers_dir(&repo.name) else {
        return;
    };
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("refresh.log"));
    let Ok(log) = log else {
        return;
    };
    let Ok(log_err) = log.try_clone() else {
        return;
    };
    use std::os::unix::process::CommandExt;
    let _ = Command::new(exe)
        .args(["__layer-refresh", &repo.name])
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(log_err)
        .process_group(0)
        .spawn();
}

// -- Workspace repos --

/// Appends per-step durations to a task log (shown with --verbose).
struct StepTimer {
    last: std::time::Instant,
}

impl StepTimer {
    fn new() -> Self {
        Self {
            last: std::time::Instant::now(),
        }
    }

    fn mark(&mut self, log: &mut String, step: &str) {
        let now = std::time::Instant::now();
        log.push_str(&format!(
            "  \x1b[2m{}: {:.2}s\x1b[0m\n",
            step,
            (now - self.last).as_secs_f64()
        ));
        self.last = now;
    }
}

/// Undo a partially created layer repo unless disarmed.
struct CreateGuard<'a> {
    bare: &'a str,
    image: PathBuf,
    mount_point: PathBuf,
    admin: PathBuf,
    created_branch: Option<String>,
    armed: bool,
}

impl Drop for CreateGuard<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if detach(&self.mount_point).is_ok() {
            let _ = fs::remove_dir_all(&self.mount_point);
        }
        let _ = fs::remove_file(&self.image);
        let _ = fs::remove_dir_all(&self.admin);
        if let Some(branch) = &self.created_branch {
            let _ = Command::new("git")
                .args(["-C", self.bare, "branch", "-D", branch])
                .output();
        }
    }
}

/// Create `<ws_root>/<repo>` as a layer worktree on branch `session`.
///
/// The branch starts at the bare repo's HEAD unless it already exists, in
/// which case the existing branch is checked out (like `git worktree add`).
pub fn create_repo(
    repo: &RepoEntry,
    ws_root: &Path,
    session: &str,
    log: &mut String,
) -> Result<()> {
    ensure_supported()?;
    let bare = repo.path.as_str();
    ensure_volume_excludes(bare)?;
    let target = git_stdout(&["-C", bare, "rev-parse", "--verify", "HEAD^{commit}"])?;
    let mut timer = StepTimer::new();
    ensure_base(repo, &target, log)?;
    timer.mark(log, "base");

    let image = image_path(ws_root, &repo.name);
    let mount_point = ws_root.join(&repo.name);
    let admin = ws_admin_dir(bare, session);
    if image.exists() {
        bail!("{} already exists", image.display());
    }
    if admin.exists() {
        bail!(
            "git worktree entry {} already exists; remove it before retrying",
            admin.display()
        );
    }
    if mount_point.exists() && fs::read_dir(&mount_point)?.next().is_some() {
        bail!("{} already exists and is not empty", mount_point.display());
    }
    fs::create_dir_all(image.parent().unwrap())?;

    let mut guard = CreateGuard {
        bare,
        image: image.clone(),
        mount_point: mount_point.clone(),
        admin: admin.clone(),
        created_branch: None,
        armed: true,
    };

    // Clone the base image and its index while holding the shared lock so a
    // concurrent publish can't delete them mid-copy.
    let staged_index = ws_root
        .join(IMAGES_DIR)
        .join(format!("{}.index", repo.name));
    let base_commit = {
        let dir = layers_dir(&repo.name)?;
        let lock = open_lock(&dir.join("lock"))?;
        lock.lock_shared()?;
        let base = current_base(&dir)
            .ok_or_else(|| anyhow::anyhow!("base image for '{}' disappeared", repo.name))?;
        fs::copy(&base.image, &image)?;
        fs::copy(&base.index, &staged_index)?;
        base.commit
    };
    timer.mark(log, "clone image");

    attach(&image, &mount_point)?;
    timer.mark(log, "attach");

    let branch_ref = format!("refs/heads/{}", session);
    let branch_exists = Command::new("git")
        .args(["-C", bare, "show-ref", "--verify", "--quiet", &branch_ref])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if branch_exists {
        if checked_out_elsewhere(bare, session) {
            bail!(
                "branch '{}' is already checked out in another worktree",
                session
            );
        }
    } else {
        git_logged(&["-C", bare, "branch", "--no-track", session, &target], log)?;
        guard.created_branch = Some(session.to_string());
    }

    write_admin(
        &admin,
        &mount_point,
        &format!("ref: {}", branch_ref),
        Some(&staged_index),
        true,
    )?;
    let _ = fs::remove_file(&staged_index);

    let mp = path_str(&mount_point)?;
    // The index matches the base commit; a hard reset rewrites only the files
    // that differ between the base and the branch tip.
    git_logged(&["-C", mp, "reset", "-q", "--hard"], log)?;
    crate::workspace::set_self_upstream(mp, session);
    guard.armed = false;
    timer.mark(log, "register + checkout");

    // Run post-checkout in the final location with the arguments `git
    // worktree add` uses. A failing hook fails creation but keeps the
    // workspace for inspection, matching worktree behavior.
    let tip = git_stdout(&["-C", mp, "rev-parse", "HEAD"])?;
    let zero = zero_oid(mp);
    let hook = git_logged(
        &[
            "-C",
            mp,
            "hook",
            "run",
            "--ignore-missing",
            "post-checkout",
            "--",
            &zero,
            &tip,
            "1",
        ],
        log,
    );
    timer.mark(log, "post-checkout hook");

    if base_commit != target {
        schedule_refresh(repo);
    }
    hook.context("post-checkout hook failed")
}

/// Attach the workspace image for `repo_name` if it isn't mounted.
/// Returns true if it attached.
pub fn ensure_mounted(ws_root: &Path, repo_name: &str) -> Result<bool> {
    let mount_point = ws_root.join(repo_name);
    if is_mount_point(&mount_point) {
        return Ok(false);
    }
    let image = image_path(ws_root, repo_name);
    if !image.is_file() {
        bail!("disk image {} is missing", image.display());
    }
    if mount_point.exists() && fs::read_dir(&mount_point)?.next().is_some() {
        bail!(
            "{} is not empty; refusing to mount over it",
            mount_point.display()
        );
    }
    attach(&image, &mount_point)?;
    Ok(true)
}

/// Remove a layer repo from a workspace: detach, then delete the mount point,
/// image, and git admin dir. Branch deletion is left to the caller.
pub fn remove_repo(
    bare: Option<&str>,
    ws_root: &Path,
    repo_name: &str,
    session: &str,
) -> Result<()> {
    let mount_point = ws_root.join(repo_name);
    detach(&mount_point)?;
    if is_mount_point(&mount_point) {
        bail!("{} is still mounted", mount_point.display());
    }
    remove_dir_all_if_exists(&mount_point)?;
    remove_file_if_exists(&image_path(ws_root, repo_name))?;
    remove_file_if_exists(
        &ws_root
            .join(IMAGES_DIR)
            .join(format!("{}.index", repo_name)),
    )?;
    if let Some(bare) = bare {
        remove_dir_all_if_exists(&ws_admin_dir(bare, session))?;
    }
    let _ = fs::remove_dir(ws_root.join(IMAGES_DIR));
    Ok(())
}

/// Delete all base images for a repo (used when the source is removed).
pub fn remove_repo_layers(repo_name: &str) -> Result<()> {
    let dir = layers_dir(repo_name)?;
    if !dir.exists() {
        return Ok(());
    }
    let build = open_lock(&dir.join("build.lock"))?;
    build.lock()?;
    if let Ok(entries) = fs::read_dir(dir.join("staging")) {
        for entry in entries.flatten() {
            detach(&entry.path())?;
        }
    }
    drop(build);
    remove_dir_all_if_exists(&dir)
}

/// Return the first direct child of `dir` that is a mount point.
pub fn find_mounted_child(dir: &Path) -> Option<PathBuf> {
    fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| is_mount_point(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_with_volume_excludes_appends_block_once() {
        let once = with_volume_excludes("*.log");
        assert!(once.starts_with("*.log\n"));
        assert!(once.contains("/.fseventsd/\n"));
        assert!(once.contains(EXCLUDE_END));
        assert_eq!(with_volume_excludes(&once), once);
    }

    #[test]
    fn test_with_volume_excludes_empty() {
        let out = with_volume_excludes("");
        assert!(out.starts_with(EXCLUDE_BEGIN));
    }

    #[test]
    fn test_admin_prefixes_are_disjoint() {
        // A session admin dir must never match the build-admin cleanup prefix.
        let session_admin = format!("{}{}", WS_ADMIN_PREFIX, "build-1-2");
        assert!(!session_admin.starts_with(BUILD_ADMIN_PREFIX));
    }

    #[test]
    fn test_is_mount_point() {
        let tmp = tempfile::tempdir().unwrap();
        let child = tmp.path().join("child");
        fs::create_dir(&child).unwrap();
        assert!(!is_mount_point(&child));
        assert!(!is_mount_point(&tmp.path().join("missing")));
        assert!(is_mount_point(Path::new("/dev")) || cfg!(not(unix)));
    }

    #[test]
    fn test_write_admin_files() {
        let tmp = tempfile::tempdir().unwrap();
        let admin = tmp.path().join("admin");
        let mp = tmp.path().join("mp");
        fs::create_dir(&mp).unwrap();
        write_admin(&admin, &mp, "ref: refs/heads/x", None, true).unwrap();
        assert_eq!(
            fs::read_to_string(admin.join("gitdir")).unwrap(),
            format!("{}\n", mp.join(".git").display())
        );
        assert_eq!(
            fs::read_to_string(admin.join("commondir")).unwrap(),
            "../..\n"
        );
        assert_eq!(
            fs::read_to_string(admin.join("HEAD")).unwrap(),
            "ref: refs/heads/x\n"
        );
        assert!(admin.join("locked").exists());
        assert_eq!(
            fs::read_to_string(mp.join(".git")).unwrap(),
            format!("gitdir: {}\n", admin.display())
        );
    }
}
