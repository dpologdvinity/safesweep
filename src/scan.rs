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
        Matcher::NamedDirs(name) => {
            let mut out = Vec::new();
            let mut walk = WalkDir::new(root)
                .follow_links(false)
                .same_file_system(true)
                .min_depth(1)
                .into_iter();
            while let Some(entry) = walk.next() {
                let Ok(entry) = entry else { continue };
                if entry.file_type().is_dir() && entry.file_name() == *name {
                    out.push(entry.into_path());
                    walk.skip_current_dir();
                }
            }
            out
        }
    }
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
        let md = entry?.metadata()?;
        if md.dev() != top.dev() {
            return Err(io::Error::other("contains a mount point"));
        }
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
pub fn mount_points() -> io::Result<Vec<PathBuf>> {
    Ok(fs::read_to_string("/proc/self/mountinfo")?
        .lines()
        .filter_map(|l| l.split(' ').nth(4))
        .map(|m| PathBuf::from(unescape(m)))
        .collect())
}

/// Undo mountinfo's octal escapes (`\040` is a space).
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('\\') {
        out.push_str(&rest[..i]);
        let code = rest
            .get(i + 1..i + 4)
            .and_then(|o| u8::from_str_radix(o, 8).ok());
        match code {
            Some(b) => {
                out.push(b as char);
                rest = &rest[i + 4..];
            }
            None => {
                out.push('\\');
                rest = &rest[i + 1..];
            }
        }
    }
    out + rest
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
}
