//! The safety guard: the one place that decides whether a candidate may be deleted.
//!
//! [`check`] takes what [`crate::scan::measure`] observed about a candidate ([`Entry`]) and
//! returns `Ok(())` or the reason to skip it. Every rule is checked here and nowhere else, and
//! `clean` runs the same check again immediately before each removal, so a candidate can only
//! be deleted if it passes at the last possible moment.
//!
//! The guard is deliberately one-sided. It can say "no" for a wrong reason (a candidate is
//! skipped that could have been deleted); it must never say "yes" for a wrong reason. When
//! something cannot be verified, the answer is "no".
//!
//! # What "in use" means
//!
//! A candidate is in use if any running process of the current user has a file inside it open,
//! has it mapped into memory, is running a program from it, or has it as its working
//! directory. [`in_use`] reads that from `/proc` once, and [`check`] matches candidates against
//! it by path and by inode (see [`InUse`] for why both).

use crate::Ctx;
use crate::scan::Entry;
use std::collections::HashSet;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// What running processes are using, gathered once from `/proc`.
///
/// The same set of files is recorded two ways, because each catches what the other misses:
///
/// - **By path**, as the process sees it. Simple, and works for ordinary processes.
/// - **By inode** (device number plus inode number). A process in another mount namespace,
///   such as a container, a `systemd` service with a private `/tmp`, or `unshare -m`, writes
///   paths from its own point of view (`/app/...`), which never match the host's paths. The
///   inode is the same however the file is reached.
pub struct InUse {
    /// Target of every open file, working directory, executable and memory-mapped file, as text
    /// in each process's own view.
    pub paths: Vec<PathBuf>,
    /// The same objects as `(device, inode)` pairs.
    pub inodes: HashSet<(u64, u64)>,
}

/// Reads everything that running processes have open, mapped, executing, or as their working
/// directory, from the process tree in `proc_root` (normally `/proc`).
///
/// Processes that cannot be inspected (other users' processes when not running as root, or
/// ones that exit while being read) are skipped silently: they cannot be seen, and the tool
/// documents that it only protects against processes it can see.
///
/// # Errors
///
/// Returns an error only if `proc_root` itself cannot be read. Callers must then treat every
/// candidate as unverifiable, because "nothing is open" cannot be told from "cannot look".
pub fn in_use(proc_root: &Path) -> io::Result<InUse> {
    let mut out = InUse {
        paths: Vec::new(),
        inodes: HashSet::new(),
    };
    let link = |p: PathBuf, out: &mut InUse| {
        out.paths.extend(fs::read_link(&p));
        // stat on a /proc magic link follows it to the real object, whatever namespace it lives in
        if let Ok(md) = fs::metadata(&p) {
            out.inodes.insert((md.dev(), md.ino()));
        }
    };
    for pid in fs::read_dir(proc_root)?.flatten() {
        if !pid
            .file_name()
            .to_string_lossy()
            .bytes()
            .all(|b| b.is_ascii_digit())
        {
            continue; // not a process directory (/proc/stat, /proc/self, ...)
        }
        let dir = pid.path();
        for fd in fs::read_dir(dir.join("fd")).into_iter().flatten().flatten() {
            link(fd.path(), &mut out);
        }
        link(dir.join("cwd"), &mut out);
        link(dir.join("exe"), &mut out);
        if let Ok(maps) = fs::read_to_string(dir.join("maps")) {
            for line in maps.lines() {
                out.paths.extend(map_path(line));
                out.inodes.extend(map_inode(line));
            }
        }
    }
    Ok(out)
}

/// The `(device, inode)` pair on one line of `/proc/<pid>/maps`, or `None` for anonymous
/// mappings (inode 0, such as the heap) and malformed lines.
///
/// A line looks like `7f00-7f01 r--p 00000000 08:20 1234   /path/to/lib.so`. The device is
/// field 4 as `major:minor` in hexadecimal, the inode is field 5.
fn map_inode(line: &str) -> Option<(u64, u64)> {
    let mut f = line.split_whitespace().skip(3);
    let (major, minor) = f.next()?.split_once(':')?;
    let ino: u64 = f.next()?.parse().ok()?;
    let (major, minor) = (
        u32::from_str_radix(major, 16).ok()?,
        u32::from_str_radix(minor, 16).ok()?,
    );
    (ino != 0).then(|| (libc::makedev(major, minor) as u64, ino))
}

/// The file path on one line of `/proc/<pid>/maps`, or `None` if the mapping is not a file
/// (`[heap]`, `[stack]`, anonymous memory).
///
/// The path is everything after the fifth field, so it may contain spaces. A trailing
/// ` (deleted)` that the kernel appends for an unlinked file is removed, so a library that was
/// deleted while still mapped still matches its old location.
fn map_path(line: &str) -> Option<PathBuf> {
    let mut rest = line;
    for _ in 0..5 {
        rest = rest.trim_start().split_once(char::is_whitespace)?.1;
    }
    let path = rest.trim_start();
    let path = path.strip_suffix(" (deleted)").unwrap_or(path);
    path.starts_with('/').then(|| PathBuf::from(path))
}

/// Decides whether the candidate `path` may be deleted: `Ok(())` if it may, otherwise
/// `Err(reason)` with a short message that is shown to the user as the skip reason.
///
/// The checks run in this order and the first failure wins:
///
/// 1. The candidate is not a symbolic link.
/// 2. The candidate lies inside its rule's root directory after resolving symlinks in the
///    path. As root, the root directory must also not itself be reached through a symlink.
/// 3. As root, the candidate is not in the temporary directory (see below).
/// 4. As an ordinary user, every file in the tree belongs to that user.
/// 5. Nothing in the tree changed within `min_age_days`.
/// 6. The tree holds no socket or named pipe.
/// 7. The open-file information is available (`in_use` is `Some`).
/// 8. No running process uses the tree, or `scope` (below), by path or by inode.
///
/// # Arguments
///
/// - `path`: the candidate. It is never followed if it is a symlink.
/// - `scope`: the directory that must be unused by any process. Usually the same as `path`.
///   For build directories it is the whole project: a running server has its working directory
///   in the project, not inside `node_modules`, yet deleting its dependencies would break it.
/// - `entry`: what [`crate::scan::measure`] observed about `path`.
/// - `root`: the rule's root directory that `path` must stay inside.
/// - `min_age_days`: how long the whole tree must have been unchanged.
/// - `ctx`: the environment (current time, user id, temp directory).
/// - `in_use`: what processes are using, from [`in_use`]; `None` if `/proc` was unreadable.
///
/// # Why root is treated differently
///
/// Root skips the owner check (it may delete anything), so everything else must be stricter:
/// the rule root may not be a symlink, because a user-controlled `~/.cache -> /var/lib` would
/// otherwise turn system directories into "cache entries". The temporary directory is refused
/// outright, because for root `/proc` shows paths from containers and services with a private
/// `/tmp` in their own view, so their live files cannot be recognised there.
pub fn check(
    path: &Path,
    scope: &Path,
    entry: &Entry,
    root: &Path,
    min_age_days: u64,
    ctx: &Ctx,
    in_use: Option<&InUse>,
) -> Result<(), String> {
    if entry.is_symlink {
        return Err("symlink".into());
    }
    let canon = canonical(path).map_err(|e| format!("cannot resolve path: {e}"))?;
    let resolved = fs::canonicalize(root).map_err(|e| format!("cannot resolve rule root: {e}"))?;
    // As root nothing else bounds deletion, so the rule root must be where it says it is.
    if ctx.euid == 0 && resolved != root {
        return Err("rule root is a symlink (refused as root)".into());
    }
    let root = resolved;
    if !canon.starts_with(&root) {
        return Err("outside rule root".into());
    }
    // As root, /proc shows PrivateTmp services' and containers' paths in their own mount
    // namespace, so their live tmp files cannot be matched against host paths.
    if ctx.euid == 0 && fs::canonicalize(&ctx.tmp).is_ok_and(|t| canon.starts_with(t)) {
        return Err("tmp is not cleaned as root".into());
    }
    if ctx.euid != 0 && entry.owner != Some(ctx.euid) {
        return Err("owned by another user".into());
    }
    let age = ctx.now.duration_since(entry.newest).unwrap_or_default();
    if age < Duration::from_secs(min_age_days * 86400) {
        return Err(format!("modified within {min_age_days} days"));
    }
    if entry.has_special {
        return Err("contains socket or fifo".into());
    }
    let Some(in_use) = in_use else {
        return Err("cannot verify open files".into());
    };
    let scope = canonical(scope).map_err(|e| format!("cannot resolve path: {e}"))?;
    let scope_inode = fs::metadata(&scope).ok().map(|m| (m.dev(), m.ino()));
    let by_path = in_use
        .paths
        .iter()
        .any(|p| p.starts_with(&canon) || p.starts_with(&scope));
    let by_inode = scope_inode.is_some_and(|i| in_use.inodes.contains(&i))
        || entry.inodes.iter().any(|i| in_use.inodes.contains(i));
    if by_path || by_inode {
        return Err("in use by a process".into());
    }
    Ok(())
}

/// Resolves symbolic links in the parent directory of `path` and appends the final name
/// unchanged, so the candidate itself is never followed even if it is a symlink.
fn canonical(path: &Path) -> io::Result<PathBuf> {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => Ok(fs::canonicalize(parent)?.join(name)),
        _ => fs::canonicalize(path),
    }
}

#[cfg(test)]
/// Unit tests. Each builds a candidate in a temporary directory and runs the real [`check`]
/// against real `/proc` data (open files, working directories of child processes).
mod tests {
    use super::*;
    use crate::scan::measure;
    use filetime::{FileTime, set_file_mtime};
    use std::fs::{self, File};
    use std::os::unix::fs::symlink;
    use std::os::unix::net::UnixListener;
    use std::process::Command;
    use std::time::{Duration, SystemTime};

    const DAY: Duration = Duration::from_secs(86400);

    /// "Now" is 10 days ahead: ctime cannot be backdated, so fixtures age by moving the clock.
    fn ctx() -> Ctx {
        Ctx {
            home: PathBuf::new(),
            tmp: PathBuf::new(),
            proc_root: PathBuf::from("/proc"),
            euid: unsafe { libc::geteuid() },
            now: SystemTime::now() + DAY * 10,
        }
    }

    /// Root dir containing candidate `c/f`.
    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let c = t.path().join("c");
        fs::create_dir(&c).unwrap();
        fs::write(c.join("f"), "x").unwrap();
        (t, c)
    }

    fn run(c: &Path, root: &Path, days: u64, ctx: &Ctx) -> Result<(), String> {
        let live = in_use(&ctx.proc_root).unwrap();
        check(c, c, &measure(c).unwrap(), root, days, ctx, Some(&live))
    }

    #[test]
    fn ok_when_old_and_unused() {
        let (t, c) = fixture();
        assert_eq!(run(&c, t.path(), 7, &ctx()), Ok(()));
    }

    #[test]
    fn skips_symlink() {
        let (t, c) = fixture();
        let link = t.path().join("link");
        symlink(&c, &link).unwrap();
        assert_eq!(run(&link, t.path(), 7, &ctx()), Err("symlink".into()));
    }

    #[test]
    fn skips_outside_root() {
        let t = tempfile::tempdir().unwrap();
        let (_o, outside_c) = fixture();
        symlink(outside_c.parent().unwrap(), t.path().join("sub")).unwrap();
        let c = t.path().join("sub/c");
        assert_eq!(
            run(&c, t.path(), 7, &ctx()),
            Err("outside rule root".into())
        );
    }

    #[test]
    fn skips_foreign_owner() {
        let (t, c) = fixture();
        let mut other = ctx();
        other.euid += 1;
        assert_eq!(
            run(&c, t.path(), 7, &other),
            Err("owned by another user".into())
        );
        let mut root = ctx();
        root.euid = 0;
        assert_eq!(run(&c, t.path(), 7, &root), Ok(()));
    }

    #[test]
    fn skips_recent() {
        let (t, c) = fixture();
        let mut today = ctx();
        today.now = SystemTime::now();
        assert_eq!(
            run(&c, t.path(), 7, &today),
            Err("modified within 7 days".into())
        );
    }

    #[test]
    fn skips_backdated_mtime() {
        // tar, cp -p and rsync -a keep old mtimes; ctime still shows the extraction
        let (t, c) = fixture();
        let old = FileTime::from_system_time(SystemTime::now() - DAY * 30);
        set_file_mtime(c.join("f"), old).unwrap();
        set_file_mtime(&c, old).unwrap();
        let mut today = ctx();
        today.now = SystemTime::now();
        assert_eq!(
            run(&c, t.path(), 7, &today),
            Err("modified within 7 days".into())
        );
    }

    #[test]
    fn skips_tmp_as_root() {
        // as root, /proc paths of PrivateTmp services and containers don't match host paths
        let (t, c) = fixture();
        let mut root = ctx();
        root.euid = 0;
        root.tmp = t.path().into();
        assert_eq!(
            run(&c, t.path(), 7, &root),
            Err("tmp is not cleaned as root".into())
        );
    }

    #[test]
    fn map_path_keeps_spaces() {
        let line = "7f00-7f01 r--p 00000000 08:20 1234      /t/a b/lib.so (deleted)";
        assert_eq!(map_path(line), Some(PathBuf::from("/t/a b/lib.so")));
        assert_eq!(
            map_path("7f00-7f01 r--p 00000000 00:00 0      [heap]"),
            None
        );
        assert_eq!(map_path("7f00-7f01 r--p 00000000 00:00 0"), None);
    }

    #[test]
    fn skips_socket() {
        let (t, c) = fixture();
        let _l = UnixListener::bind(c.join("s")).unwrap();
        // min age 0: filetime cannot age a socket, so keep the age check out of the way
        assert_eq!(
            run(&c, t.path(), 0, &ctx()),
            Err("contains socket or fifo".into())
        );
    }

    #[test]
    fn skips_open_file() {
        let (t, c) = fixture();
        let _held = File::open(c.join("f")).unwrap();
        assert_eq!(
            run(&c, t.path(), 7, &ctx()),
            Err("in use by a process".into())
        );
    }

    #[test]
    fn skips_child_cwd() {
        let (t, c) = fixture();
        let mut child = Command::new("sleep")
            .arg("30")
            .current_dir(&c)
            .spawn()
            .unwrap();
        let r = run(&c, t.path(), 7, &ctx());
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(r, Err("in use by a process".into()));
    }

    #[test]
    fn scope_widens_in_use_check_to_project() {
        // a service whose cwd is the project (not the artifact) still needs its dependencies
        let (t, c) = fixture();
        let project = t.path().join("proj");
        fs::create_dir_all(project.join("node_modules")).unwrap();
        let mut child = Command::new("sleep")
            .arg("30")
            .current_dir(&project)
            .spawn()
            .unwrap();
        let nm = project.join("node_modules");
        let live = in_use(&PathBuf::from("/proc")).unwrap();
        let r = check(
            &nm,
            &project,
            &measure(&nm).unwrap(),
            t.path(),
            0,
            &ctx(),
            Some(&live),
        );
        child.kill().unwrap();
        child.wait().unwrap();
        let _ = c;
        assert_eq!(r, Err("in use by a process".into()));
    }

    #[test]
    fn skips_without_proc() {
        let (t, c) = fixture();
        let r = check(&c, &c, &measure(&c).unwrap(), t.path(), 7, &ctx(), None);
        assert_eq!(r, Err("cannot verify open files".into()));
    }

    #[test]
    fn in_use_errs_on_missing_proc_root() {
        let t = tempfile::tempdir().unwrap();
        assert!(in_use(&t.path().join("noproc")).is_err());
    }

    #[test]
    fn map_inode_parses_dev_and_inode() {
        let line = "7f00-7f01 r--p 00000000 08:20 1234      /t/a b/lib.so";
        assert_eq!(map_inode(line), Some((libc::makedev(8, 0x20) as u64, 1234)));
        assert_eq!(
            map_inode("7f00-7f01 r--p 00000000 00:00 0      [heap]"),
            None
        );
    }

    #[test]
    fn root_refuses_symlinked_rule_root() {
        // as root, ~/.cache -> /var/lib would make system dirs look like cache entries
        let t = tempfile::tempdir().unwrap();
        let real = t.path().join("real");
        fs::create_dir_all(real.join("c")).unwrap();
        fs::write(real.join("c/f"), "x").unwrap();
        let link = t.path().join("link");
        symlink(&real, &link).unwrap();
        let c = link.join("c");
        let mut root = ctx();
        root.euid = 0;
        assert_eq!(
            run(&c, &link, 7, &root),
            Err("rule root is a symlink (refused as root)".into())
        );
        // a normal user may symlink their own ~/.cache
        assert_eq!(run(&c, &link, 7, &ctx()), Ok(()));
    }
}
