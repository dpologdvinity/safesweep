//! Finding candidate paths and measuring them.
//!
//! This module answers two questions about the filesystem, and never changes it:
//!
//! 1. [`find`]: which paths could a rule delete? (a [`Matcher`] decides)
//! 2. [`measure`]: what is inside one candidate? Its size, newest change, owner, special
//!    files and inodes, as an [`Entry`].
//!
//! Whether a candidate is *safe* to delete is decided later, in [`crate::guard`], from the
//! [`Entry`] produced here. Keeping the two apart means this module only has to observe, and
//! the guard is the single place that says "no".
//!
//! # Conservative by design
//!
//! [`measure`] fails (returns an error) whenever it cannot see the whole tree or when deleting
//! the tree would not behave like a plain removal: an unreadable directory, a mount point, a
//! nested `.git`, or a read-only directory. A candidate that cannot be measured is skipped, never
//! guessed at. The walks never follow symlinks and never cross onto another filesystem.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

/// How a rule picks candidates under its root directory.
pub enum Matcher {
    /// The root itself is the single candidate (for example `~/.cache/pip`).
    Whole,
    /// Each direct child of the root is a candidate, except the named entries.
    ///
    /// The exclusions let one rule own a directory that another rule handles separately,
    /// such as `~/.cache/pip` being excluded from the `~/.cache` rule.
    Children {
        /// File names (not paths) to skip.
        exclude: &'static [&'static str],
    },
    /// Every directory with this exact name anywhere under the root (for example
    /// `__pycache__`). A match is never searched further, so nested copies are not listed twice.
    /// Hidden directories (names starting with `.`) are not entered.
    NamedDirs(&'static str),
    /// Build directories (`node_modules`, `target`, `.venv`) of git projects that have had no
    /// git activity for 30 days.
    ///
    /// A directory only counts if a project marker file sits next to it (`package.json`,
    /// `Cargo.toml`, ...) *and* it contains a file only the tool itself creates (`pyvenv.cfg`,
    /// `CACHEDIR.TAG`, ...), so a hand-made folder with the same name is never taken for build
    /// output. Projects without a readable `.git` directory are never listed.
    IdleArtifacts,
}

/// What [`measure`] found inside one candidate (a file, or a whole directory tree).
///
/// Every field is a plain observation; [`crate::guard::check`] turns them into a verdict.
pub struct Entry {
    /// Disk space the tree would free, in bytes: allocated blocks of regular files that have a
    /// single hard link. Files with more links are not counted, because deleting one link frees
    /// nothing (uv hardlinks its cache into virtual environments).
    pub bytes: u64,
    /// Time of the most recent change anywhere in the tree: the newer of modification time and
    /// inode change time, over every entry. Both are used because tools like `tar` and
    /// `rsync -a` set the modification time back to the original, so it alone can make a tree
    /// unpacked a minute ago look years old; the inode change time cannot be set by any program.
    pub newest: SystemTime,
    /// `Some(uid)` only if every entry in the tree belongs to that one user; `None` if owners
    /// are mixed.
    pub owner: Option<u32>,
    /// The tree contains a socket or a named pipe (FIFO). A live program may be using it even
    /// though no file path shows up as open.
    pub has_special: bool,
    /// The candidate path itself is a symbolic link (links inside the tree are not followed).
    pub is_symlink: bool,
    /// `(device, inode)` of every entry in the tree. The guard matches these against what
    /// running processes have open, which works even when a process sees the path differently
    /// (containers, other mount namespaces).
    pub inodes: HashSet<(u64, u64)>,
}

/// Returns the candidate paths that `m` selects under `root`.
///
/// A `root` that does not exist or cannot be read yields an empty list rather than an error: a
/// cache directory that was never created simply has nothing to clean. Order is unspecified.
/// Symbolic links are not followed, so a link never leads the walk outside `root`.
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
                // an unreadable directory must not end the whole walk
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

/// The build directories [`Matcher::IdleArtifacts`] recognises.
///
/// Each entry is `(directory name, project marker files, tool fingerprint files)`:
///
/// - the directory must sit next to at least one *marker file* (for example `package.json` next
///   to `node_modules`), so an unrelated folder that happens to be called `target` is ignored;
/// - the directory must contain at least one *fingerprint file* that only the tool itself
///   creates (for example `pyvenv.cfg` inside a virtual environment). Without this, a hand-made
///   `.venv` holding someone's code, such as the editable checkouts pip puts in `.venv/src`,
///   would be mistaken for build output and deleted.
const ARTIFACTS: [(&str, &[&str], &[&str]); 3] = [
    (
        "node_modules",
        &["package.json"],
        &[
            ".package-lock.json",
            ".yarn-integrity",
            ".yarn-state.yml",
            ".modules.yaml",
            ".bun", // bun --linker isolated; bun's default layout has no marker
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

/// A project counts as idle after this many days without git activity.
const IDLE_DAYS: u64 = 30;

/// How many directory levels below the root [`idle_artifacts`] searches.
const MAX_DEPTH: usize = 6;

/// Walks `root` for build directories of idle git projects (see [`Matcher::IdleArtifacts`]).
///
/// Hidden directories are skipped (except `.venv`, which is itself a candidate name), and a
/// directory that is not a real `node_modules` is not entered, which keeps the walk fast on
/// large trees. Once a build directory is found, its contents are not searched.
///
/// Idleness is judged against the real clock, not [`crate::Ctx::now`]. This is a shortcut:
/// tests cannot fake it, so they back-date the git files instead.
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
        if is_artifact(&entry) {
            walk.skip_current_dir();
            if project_idle(entry.path().parent().unwrap_or(root)) {
                out.push(entry.into_path());
            }
        } else if entry.file_name().as_encoded_bytes().starts_with(b".")
            || entry.file_name() == "node_modules"
        {
            walk.skip_current_dir();
        }
    }
    out
}

/// True if `entry` is a build directory of a known tool: right name, a project marker file next
/// to it, and a fingerprint file inside it. See [`ARTIFACTS`].
fn is_artifact(entry: &walkdir::DirEntry) -> bool {
    let Some(project) = entry.path().parent() else {
        return false;
    };
    let name = entry.file_name();
    ARTIFACTS.iter().any(|(dir, markers, fingerprints)| {
        *dir == name
            && markers.iter().any(|m| project.join(m).is_file())
            && fingerprints.iter().any(|f| entry.path().join(f).exists())
    })
}

/// Time of the newest git activity in the repository whose `.git` directory is `git`, or `None`
/// if there is none to read (not a git directory, or `.git` is a file as in a linked worktree).
///
/// It takes the newest modification time of the files git touches on commit, checkout and
/// fetch: the main checkout, every branch's reflog, and every linked worktree. Worktrees
/// matter because commits made there never touch the main checkout's `logs/HEAD`.
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

/// True if the project's last git activity is older than [`IDLE_DAYS`].
///
/// A project with no readable `.git` directory has unknown activity, and unknown is treated as
/// *not* idle: the tool only acts on projects it can positively show to be abandoned.
fn project_idle(project: &Path) -> bool {
    let activity = git_activity(&project.join(".git"));
    activity.is_some_and(|t| {
        SystemTime::now().duration_since(t).unwrap_or_default()
            > Duration::from_secs(IDLE_DAYS * 86400)
    })
}

/// Measures the tree at `path`, reading the mount table itself.
///
/// This is [`measure_with`] for a single path. When measuring many paths, read the mount table
/// once with [`mount_points`] and call [`measure_with`], which is what a scan does.
///
/// # Errors
///
/// Returns an error if the tree cannot be fully seen or would not delete cleanly: any entry
/// that cannot be read, a mount point at or below `path`, a `.git` entry anywhere inside, or a
/// directory without owner write permission. A tree we cannot fully see is not safe to delete.
pub fn measure(path: &Path) -> io::Result<Entry> {
    measure_with(path, &mount_points()?)
}

/// Measures the tree at `path` against an already-read mount table (`mounts`).
///
/// If `path` is itself a symbolic link, the link is described and not followed: the returned
/// [`Entry`] has `is_symlink` set and nothing else filled in.
///
/// # Errors
///
/// See [`measure`]. Each refusal has a short reason as the error message, which is shown to the
/// user as the "skipped" reason.
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
    // Comparing device numbers alone misses a bind mount from the same filesystem and a
    // candidate that is itself a mount point, so the mount table is checked as well.
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

/// The time of the last change to a file: the newer of its modification time and its inode
/// change time.
///
/// Both are needed. Tools like `tar`, `cp -p` and `rsync -a` set the modification time back to
/// the original, so a tree unpacked a minute ago can look years old by that clock. The inode
/// change time cannot be set by any program, so it always reflects when the file really arrived.
fn changed(md: &fs::Metadata) -> io::Result<SystemTime> {
    let ctime = UNIX_EPOCH + Duration::new(md.ctime().max(0) as u64, md.ctime_nsec() as u32);
    Ok(md.modified()?.max(ctime))
}

/// Extracts the mount points from the bytes of `/proc/self/mountinfo`.
///
/// The file is read as bytes, not text: a single mount point whose path is not valid UTF-8
/// must not make every candidate impossible to verify. The mount point is the fifth
/// space-separated field of each line, with spaces and other special characters written as
/// octal escapes (see [`unescape`]).
fn parse_mountinfo(info: &[u8]) -> Vec<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    info.split(|&b| b == b'\n')
        .filter_map(|l| l.split(|&b| b == b' ').nth(4))
        .map(|m| PathBuf::from(std::ffi::OsString::from_vec(unescape(m))))
        .collect()
}

/// Every mount point on the system, read from `/proc/self/mountinfo`.
///
/// # Errors
///
/// Returns an error if the file cannot be read, for example when `/proc` is not mounted. The
/// caller must then treat candidates as unverifiable.
pub fn mount_points() -> io::Result<Vec<PathBuf>> {
    Ok(parse_mountinfo(&fs::read("/proc/self/mountinfo")?))
}

/// Decodes the octal escapes the kernel uses in `mountinfo` paths: `\040` is a space, `\011` a
/// tab, `\134` a backslash. Anything else, including a lone backslash, is kept as is.
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
/// Unit tests. Every test builds its own tree in a temporary directory and never touches the
/// real home directory. File ages are set with `filetime`; trees are "aged" by moving the clock
/// instead where the inode change time (which cannot be back-dated) is involved.
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
    fn idle_artifacts_skips_non_artifact_venv_and_does_not_descend() {
        let t = tempfile::tempdir().unwrap();
        // A .venv that is not a real artifact (e.g. hand-made, no pyvenv.cfg)
        let venv = t.path().join("myproj/.venv");
        fs::create_dir_all(&venv).unwrap();
        // A nested project inside the non-artifact .venv should not be discovered because
        // hidden directories and build dirs must not be descended into
        project(&venv, "nested", "package.json", "node_modules", Some(60));
        assert!(find(t.path(), &Matcher::IdleArtifacts).is_empty());
    }

    #[test]
    fn idle_artifacts_recognises_bun_isolated_store() {
        // `bun install --linker isolated` creates node_modules/.bun (its package store); checked
        // against bun 1.4.2. Bun's default layout writes no marker at all, so it is not recognised.
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("bun_app");
        fs::create_dir_all(p.join("node_modules")).unwrap();
        fs::write(p.join("package.json"), "x").unwrap();
        fs::create_dir(p.join("node_modules/.bun")).unwrap();
        fs::create_dir_all(p.join(".git/logs")).unwrap();
        let old = FileTime::from_system_time(SystemTime::now() - Duration::from_secs(86400 * 60));
        fs::write(p.join(".git/index"), "x").unwrap();
        set_file_mtime(p.join(".git/index"), old).unwrap();
        assert_eq!(
            find(t.path(), &Matcher::IdleArtifacts),
            vec![p.join("node_modules")]
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
