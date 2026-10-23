use crate::Ctx;
use crate::scan::Entry;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Every path a process holds open, maps, runs, or uses as its cwd.
/// Errs only if `proc_root` itself is unreadable; unreadable processes are ignored.
pub fn in_use(proc_root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for pid in fs::read_dir(proc_root)?.flatten() {
        if !pid
            .file_name()
            .to_string_lossy()
            .bytes()
            .all(|b| b.is_ascii_digit())
        {
            continue;
        }
        let dir = pid.path();
        for fd in fs::read_dir(dir.join("fd")).into_iter().flatten().flatten() {
            out.extend(fs::read_link(fd.path()));
        }
        out.extend(fs::read_link(dir.join("cwd")));
        out.extend(fs::read_link(dir.join("exe")));
        if let Ok(maps) = fs::read_to_string(dir.join("maps")) {
            out.extend(maps.lines().filter_map(map_path));
        }
    }
    Ok(out)
}

/// Path column of one `/proc/<pid>/maps` line, if it names a file.
/// The path is everything after the 5th field, so it may contain spaces.
fn map_path(line: &str) -> Option<PathBuf> {
    let mut rest = line;
    for _ in 0..5 {
        rest = rest.trim_start().split_once(char::is_whitespace)?.1;
    }
    let path = rest.trim_start();
    let path = path.strip_suffix(" (deleted)").unwrap_or(path);
    path.starts_with('/').then(|| PathBuf::from(path))
}

/// `Ok(())` if the candidate is safe to delete, else the reason to skip it.
/// `scope` is the directory that must be unused by any process: the candidate itself, or for
/// build artifacts their project (a running service has its cwd there, not inside `node_modules`).
pub fn check(
    path: &Path,
    scope: &Path,
    entry: &Entry,
    root: &Path,
    min_age_days: u64,
    ctx: &Ctx,
    in_use: Option<&[PathBuf]>,
) -> Result<(), String> {
    if entry.is_symlink {
        return Err("symlink".into());
    }
    let canon = canonical(path).map_err(|e| format!("cannot resolve path: {e}"))?;
    let root = fs::canonicalize(root).map_err(|e| format!("cannot resolve rule root: {e}"))?;
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
    if in_use
        .iter()
        .any(|p| p.starts_with(&canon) || p.starts_with(&scope))
    {
        return Err("in use by a process".into());
    }
    Ok(())
}

/// Resolves symlinks in the parent only, so the candidate itself is never followed.
fn canonical(path: &Path) -> io::Result<PathBuf> {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => Ok(fs::canonicalize(parent)?.join(name)),
        _ => fs::canonicalize(path),
    }
}

#[cfg(test)]
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
}
