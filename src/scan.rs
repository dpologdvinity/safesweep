use std::fs;
use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

/// Which paths under a rule root are candidates.
pub enum Matcher {
    /// The root itself.
    Whole,
    /// Each direct child of the root, except the named ones.
    Children { exclude: &'static [&'static str] },
    /// Every directory with this name, without descending into matches.
    NamedDirs(&'static str),
    /// Build directories (`node_modules`, `target`, `.venv`) of git projects idle for 30+ days.
    IdleArtifacts,
}

pub struct Entry {
    /// Allocated bytes of regular files with a single hard link.
    pub bytes: u64,
    /// Newest mtime of any entry in the tree.
    pub newest: SystemTime,
    /// `Some(uid)` only if every entry in the tree has that owner.
    pub owner: Option<u32>,
    /// Tree contains a socket or FIFO.
    pub has_special: bool,
    /// The top path itself is a symlink.
    pub is_symlink: bool,
    /// `(dev, inode)` of every entry, to match against what processes have open.
    pub inodes: std::collections::HashSet<(u64, u64)>,
}

/// Candidate paths under `root`. A missing or unreadable root yields none.
pub fn find(root: &Path, m: &Matcher) -> Vec<PathBuf> {
    if fs::symlink_metadata(root).is_err() {
        return Vec::new();
    }
    match m {
        Matcher::Whole => vec![root.to_path_buf()],
        Matcher::Children { exclude } => fs::read_dir(root)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| !exclude.iter().any(|x| e.file_name() == *x))
            .map(|e| e.path())
            .collect(),
        Matcher::IdleArtifacts => idle_artifacts(root),
        Matcher::NamedDirs(name) => {
            let mut out = Vec::new();
            let mut walk = WalkDir::new(root)
                .follow_links(false)
                .same_file_system(true)
                .min_depth(1)
                .into_iter();
            while let Some(entry) = walk.next() {
                let Ok(entry) = entry else { continue };
                if !entry.file_type().is_dir() {
                    continue;
                }
                if entry.file_name() == *name {
                    out.push(entry.into_path());
                    walk.skip_current_dir();
                } else if entry.file_name().as_encoded_bytes().starts_with(b".") {
                    // .cache, .rustup, .npm, ...: other rules own those, and walking them is slow
                    walk.skip_current_dir();
                }
            }
            out
        }
    }
}

/// Directory name -> files that must sit next to it for it to count as that tool's artifact.
/// Each also needs a file inside that only the tool itself creates, so a user's own directory
/// of the same name (or `.venv/src/<pkg>` editable checkouts' parent) is never taken for build output.
const ARTIFACTS: [(&str, &[&str], &[&str]); 3] = [
    (
        "node_modules",
        &["package.json"],
        &[
            ".package-lock.json",
            ".yarn-integrity",
            ".yarn-state.yml",
            ".modules.yaml",
        ],
    ),
    (
        "target",
        &["Cargo.toml"],
        &["CACHEDIR.TAG", ".rustc_info.json"],
    ),
    (
        ".venv",
        &["pyproject.toml", "requirements.txt", "setup.py"],
        &["pyvenv.cfg"],
    ),
];
const IDLE_DAYS: u64 = 30;
const MAX_DEPTH: usize = 6;

// shortcut: idleness uses the real clock, not Ctx.now; thread Ctx through find if tests need to fake it.
fn idle_artifacts(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut walk = WalkDir::new(root)
        .follow_links(false)
        .same_file_system(true)
        .min_depth(1)
        .max_depth(MAX_DEPTH)
        .into_iter();
    while let Some(entry) = walk.next() {
        // an unreadable directory must not end the whole walk
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy();
        if name.starts_with('.') && name != ".venv"
            || name == "node_modules" && !is_artifact(&entry)
        {
            walk.skip_current_dir();
        } else if is_artifact(&entry) {
            walk.skip_current_dir();
            if project_idle(entry.path().parent().unwrap_or(root)) {
                out.push(entry.into_path());
            }
        }
    }
    out
}

fn is_artifact(entry: &walkdir::DirEntry) -> bool {
    let Some(project) = entry.path().parent() else {
        return false;
    };
    let name = entry.file_name().to_string_lossy();
    ARTIFACTS.iter().any(|(dir, markers, fingerprints)| {
        *dir == name
            && markers.iter().any(|m| project.join(m).is_file())
            && fingerprints.iter().any(|f| entry.path().join(f).exists())
    })
}

/// Newest mtime of the files git touches on commit, checkout and fetch: in the main checkout,
/// on any branch, and in linked worktrees (commits there never touch the main `logs/HEAD`).
fn git_activity(git: &Path) -> Option<SystemTime> {
    let mtime = |p: PathBuf| fs::metadata(p).and_then(|m| m.modified()).ok();
    let mut newest = ["logs/HEAD", "index", "FETCH_HEAD"]
        .iter()
        .filter_map(|f| mtime(git.join(f)))
        .max();
    let refs = WalkDir::new(git.join("logs/refs"))
        .into_iter()
        .flatten()
        .filter(|e| e.file_type().is_file());
    for e in refs {
        newest = newest.max(mtime(e.into_path()));
    }
    for wt in fs::read_dir(git.join("worktrees"))
        .into_iter()
        .flatten()
        .flatten()
    {
        newest = newest.max(mtime(wt.path().join("logs/HEAD")));
        newest = newest.max(mtime(wt.path().join("index")));
    }
    newest
}

/// True if the project's last git activity is older than `IDLE_DAYS`. No `.git` means unknown, so not idle.
fn project_idle(project: &Path) -> bool {
    let activity = git_activity(&project.join(".git"));
    activity.is_some_and(|t| {
        SystemTime::now().duration_since(t).unwrap_or_default()
            > Duration::from_secs(IDLE_DAYS * 86400)
    })
}

/// Size, age, ownership and special files of the tree at `path`.
/// Any unreadable entry is an error: a tree we cannot fully see is not safe to delete.
pub fn measure(path: &Path) -> io::Result<Entry> {
    measure_with(path, &mount_points()?)
}

/// `measure` against a mount table read once by the caller (a scan measures hundreds of paths).
pub fn measure_with(path: &Path, mounts: &[PathBuf]) -> io::Result<Entry> {
    let top = fs::symlink_metadata(path)?;
    let mut e = Entry {
        bytes: 0,
        newest: changed(&top)?,
        owner: Some(top.uid()),
        has_special: false,
        is_symlink: top.file_type().is_symlink(),
        inodes: Default::default(),
    };
    if e.is_symlink {
        return Ok(e);
    }
    // remove_dir_all would descend into a mounted filesystem, so refuse instead of skipping it.
    // st_dev misses bind mounts from the same filesystem and a candidate that is itself a mount.
    let canon = fs::canonicalize(path)?;
    if mounts.iter().any(|m| m.starts_with(&canon)) {
        return Err(io::Error::other("contains a mount point"));
    }
    for entry in WalkDir::new(path).follow_links(false) {
        let entry = entry?;
        // A nested checkout (editable pip install, git dependency) may hold uncommitted user work.
        if entry.file_name() == ".git" {
            return Err(io::Error::other("contains a .git directory"));
        }
        let md = entry.metadata()?;
        if md.dev() != top.dev() {
            return Err(io::Error::other("contains a mount point"));
        }
        e.inodes.insert((md.dev(), md.ino()));
        e.newest = e.newest.max(changed(&md)?);
        if e.owner != Some(md.uid()) {
            e.owner = None;
        }
        let ft = md.file_type();
        // remove_dir_all would fail partway through and leave a half-deleted tree.
        if ft.is_dir() && md.mode() & 0o200 == 0 {
            return Err(io::Error::other("contains a read-only directory"));
        }
        if ft.is_socket() || ft.is_fifo() {
            e.has_special = true;
        }
        if ft.is_file() && md.nlink() == 1 {
            e.bytes += md.blocks() * 512;
        }
    }
    Ok(e)
}

/// Newer of mtime and ctime. tar, `cp -p` and `rsync -a` backdate mtime; nobody can backdate ctime.
fn changed(md: &fs::Metadata) -> io::Result<SystemTime> {
    let ctime = UNIX_EPOCH + Duration::new(md.ctime().max(0) as u64, md.ctime_nsec() as u32);
    Ok(md.modified()?.max(ctime))
}

/// Every mount point on the system, from `/proc/self/mountinfo`.
/// Mount points from mountinfo bytes. Read as bytes: one mount point that is not valid UTF-8
/// must not make every candidate unverifiable.
fn parse_mountinfo(info: &[u8]) -> Vec<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    info.split(|&b| b == b'\n')
        .filter_map(|l| l.split(|&b| b == b' ').nth(4))
        .map(|m| PathBuf::from(std::ffi::OsString::from_vec(unescape(m))))
        .collect()
}

pub fn mount_points() -> io::Result<Vec<PathBuf>> {
    Ok(parse_mountinfo(&fs::read("/proc/self/mountinfo")?))
}

/// Undo mountinfo's octal escapes (`\040` is a space).
fn unescape(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let octal = s
            .get(i + 1..i + 4)
            .and_then(|o| std::str::from_utf8(o).ok())
            .and_then(|o| u8::from_str_radix(o, 8).ok());
        match (s[i], octal) {
            (b'\\', Some(b)) => {
                out.push(b);
                i += 4;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use filetime::{FileTime, set_file_mtime};
    use std::collections::BTreeSet;
    use std::fs;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
    use std::os::unix::net::UnixListener;
    use std::time::Duration;

    fn names(paths: Vec<PathBuf>) -> BTreeSet<String> {
        paths
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn find_missing_root_is_empty() {
        let t = tempfile::tempdir().unwrap();
        assert!(find(&t.path().join("nope"), &Matcher::Whole).is_empty());
    }

    #[test]
    fn find_children_respects_exclude() {
        let t = tempfile::tempdir().unwrap();
        for d in ["a", "pip", "b"] {
            fs::create_dir(t.path().join(d)).unwrap();
        }
        let got = names(find(t.path(), &Matcher::Children { exclude: &["pip"] }));
        assert_eq!(got, BTreeSet::from(["a".into(), "b".into()]));
    }

    #[test]
    fn find_named_dirs_no_descend() {
        let t = tempfile::tempdir().unwrap();
        let outer = t.path().join("x/__pycache__");
        fs::create_dir_all(outer.join("__pycache__")).unwrap();
        assert_eq!(
            find(t.path(), &Matcher::NamedDirs("__pycache__")),
            vec![outer]
        );
    }

    #[test]
    fn find_does_not_follow_symlinks() {
        let t = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::create_dir(outside.path().join("__pycache__")).unwrap();
        symlink(outside.path(), t.path().join("link")).unwrap();
        assert!(find(t.path(), &Matcher::NamedDirs("__pycache__")).is_empty());
    }

    #[test]
    fn measure_skips_hardlinked_bytes() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("a"), vec![1u8; 8192]).unwrap();
        fs::hard_link(t.path().join("a"), t.path().join("b")).unwrap();
        fs::write(t.path().join("c"), vec![1u8; 4096]).unwrap();
        let c_bytes = fs::metadata(t.path().join("c")).unwrap().blocks() * 512;
        assert_eq!(measure(t.path()).unwrap().bytes, c_bytes);
    }

    #[test]
    fn measure_newest_includes_ctime() {
        // mtimes backdated (as tar/cp -p/rsync -a do) must not make a fresh tree look old
        let t = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let day = Duration::from_secs(86400);
        fs::write(t.path().join("f"), "x").unwrap();
        let old = FileTime::from_system_time(now - day * 10);
        set_file_mtime(t.path().join("f"), old).unwrap();
        set_file_mtime(t.path(), old).unwrap();
        let newest = measure(t.path()).unwrap().newest;
        assert!(
            newest + Duration::from_secs(60) >= now,
            "newest is ctime, not the backdated mtime"
        );
    }

    #[test]
    fn measure_refuses_read_only_dir() {
        // remove_dir_all would fail partway through and leave a half-deleted tree
        let t = tempfile::tempdir().unwrap();
        let sub = t.path().join("ro");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("f"), "x").unwrap();
        fs::set_permissions(&sub, fs::Permissions::from_mode(0o555)).unwrap();
        let r = measure(t.path());
        fs::set_permissions(&sub, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            r.err().map(|e| e.to_string()),
            Some("contains a read-only directory".into())
        );
    }

    #[test]
    fn measure_flags_socket() {
        let t = tempfile::tempdir().unwrap();
        let _l = UnixListener::bind(t.path().join("s")).unwrap();
        assert!(measure(t.path()).unwrap().has_special);
    }

    #[test]
    fn measure_unreadable_subdir_errs() {
        if unsafe { libc::geteuid() } == 0 {
            return; // root reads everything
        }
        let t = tempfile::tempdir().unwrap();
        let sub = t.path().join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("f"), "x").unwrap();
        fs::set_permissions(&sub, fs::Permissions::from_mode(0o000)).unwrap();
        let r = measure(t.path());
        fs::set_permissions(&sub, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(r.is_err());
    }

    #[test]
    fn measure_reports_symlink_top() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("f"), vec![1u8; 4096]).unwrap();
        symlink(t.path().join("f"), t.path().join("link")).unwrap();
        let e = measure(&t.path().join("link")).unwrap();
        assert!(e.is_symlink);
        assert_eq!(e.bytes, 0);
    }

    /// Project dir with a marker file and artifact dir; git activity backdated `days`.
    fn project(
        root: &Path,
        name: &str,
        marker: &str,
        artifact: &str,
        git_days: Option<u32>,
    ) -> PathBuf {
        let p = root.join(name);
        fs::create_dir_all(p.join(artifact)).unwrap();
        // a real tool leaves a fingerprint inside; user-made dirs of the same name don't
        let fingerprint = match artifact {
            ".venv" => "pyvenv.cfg",
            "target" => "CACHEDIR.TAG",
            _ => ".package-lock.json",
        };
        fs::write(p.join(artifact).join(fingerprint), "x").unwrap();
        fs::write(p.join(marker), "x").unwrap();
        if let Some(days) = git_days {
            fs::create_dir_all(p.join(".git/logs")).unwrap();
            let old =
                FileTime::from_system_time(SystemTime::now() - Duration::from_secs(86400) * days);
            for f in [".git/logs/HEAD", ".git/index"] {
                fs::write(p.join(f), "x").unwrap();
                set_file_mtime(p.join(f), old).unwrap();
            }
        }
        p.join(artifact)
    }

    #[test]
    fn idle_artifacts_lists_only_idle_git_projects() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        let want = BTreeSet::from([
            project(r, "web", "package.json", "node_modules", Some(60)),
            project(r, "rs", "Cargo.toml", "target", Some(60)),
            project(r, "py", "pyproject.toml", ".venv", Some(60)),
            project(
                &r.join("a/b"),
                "deep",
                "package.json",
                "node_modules",
                Some(60),
            ),
        ]);
        project(r, "active", "package.json", "node_modules", Some(2));
        project(r, "nogit", "Cargo.toml", "target", None);
        project(r, "wrong-marker", "package.json", "target", Some(60));
        project(r, "dot/.hidden", "package.json", "node_modules", Some(60));
        let got: BTreeSet<PathBuf> = find(r, &Matcher::IdleArtifacts).into_iter().collect();
        assert_eq!(got, want);
    }

    #[test]
    fn idle_artifacts_does_not_descend_into_matches() {
        let t = tempfile::tempdir().unwrap();
        let nm = project(t.path(), "web", "package.json", "node_modules", Some(60));
        project(&nm, "inner", "package.json", "node_modules", Some(60));
        assert_eq!(find(t.path(), &Matcher::IdleArtifacts), vec![nm]);
    }

    #[test]
    fn idle_artifacts_requires_tool_fingerprint() {
        let t = tempfile::tempdir().unwrap();
        let ours = project(t.path(), "py", "pyproject.toml", ".venv", Some(60));
        fs::remove_file(ours.join("pyvenv.cfg")).unwrap(); // hand-made .venv: could be user code
        assert!(find(t.path(), &Matcher::IdleArtifacts).is_empty());
    }

    #[test]
    fn idle_artifacts_sees_worktree_and_branch_activity() {
        let t = tempfile::tempdir().unwrap();
        let recent = |p: PathBuf| {
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, "x").unwrap();
        };
        let a = project(t.path(), "a", "package.json", "node_modules", Some(60));
        recent(a.parent().unwrap().join(".git/worktrees/w/logs/HEAD"));
        let b = project(t.path(), "b", "package.json", "node_modules", Some(60));
        recent(b.parent().unwrap().join(".git/logs/refs/heads/feature"));
        assert!(find(t.path(), &Matcher::IdleArtifacts).is_empty());
    }

    #[test]
    fn idle_artifacts_walk_survives_unreadable_dir() {
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let t = tempfile::tempdir().unwrap();
        let bad = t.path().join("aaa");
        fs::create_dir(&bad).unwrap();
        fs::set_permissions(&bad, fs::Permissions::from_mode(0o000)).unwrap();
        let want = project(t.path(), "zzz", "package.json", "node_modules", Some(60));
        let got = find(t.path(), &Matcher::IdleArtifacts);
        fs::set_permissions(&bad, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(got, vec![want]);
    }

    #[test]
    fn measure_refuses_tree_containing_git() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir_all(t.path().join("src/pkg/.git")).unwrap();
        let r = measure(t.path());
        assert_eq!(
            r.err().map(|e| e.to_string()),
            Some("contains a .git directory".into())
        );
    }

    #[test]
    fn find_named_dirs_skips_dot_dirs() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir_all(t.path().join(".hidden/__pycache__")).unwrap();
        let shown = t.path().join("p/__pycache__");
        fs::create_dir_all(&shown).unwrap();
        assert_eq!(
            find(t.path(), &Matcher::NamedDirs("__pycache__")),
            vec![shown]
        );
    }

    #[test]
    fn parse_mountinfo_handles_escapes_and_bad_utf8() {
        use std::os::unix::ffi::OsStrExt;
        let info =
            b"36 35 98:0 / /mnt/a\\040b rw - ext3 /dev/x rw\n37 35 98:0 / /mnt/\xff rw - x y z\n";
        let got = parse_mountinfo(info);
        assert_eq!(got[0], PathBuf::from("/mnt/a b"));
        assert_eq!(got[1].as_os_str().as_bytes(), b"/mnt/\xff");
    }
}
