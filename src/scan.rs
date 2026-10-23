use std::fs;
use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
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
    let top = fs::symlink_metadata(path)?;
    let mut e = Entry {
        bytes: 0,
        newest: top.modified()?,
        owner: Some(top.uid()),
        has_special: false,
        is_symlink: top.file_type().is_symlink(),
    };
    if e.is_symlink {
        return Ok(e);
    }
    for entry in WalkDir::new(path).follow_links(false) {
        let md = entry?.metadata()?;
        // remove_dir_all would descend into a mounted filesystem, so refuse instead of skipping it.
        if md.dev() != top.dev() {
            return Err(io::Error::other("contains a mount point"));
        }
        e.newest = e.newest.max(md.modified()?);
        if e.owner != Some(md.uid()) {
            e.owner = None;
        }
        let ft = md.file_type();
        if ft.is_socket() || ft.is_fifo() {
            e.has_special = true;
        }
        if ft.is_file() && md.nlink() == 1 {
            e.bytes += md.blocks() * 512;
        }
    }
    Ok(e)
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
    fn measure_newest_is_max_mtime() {
        let t = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let day = Duration::from_secs(86400);
        fs::write(t.path().join("old"), "x").unwrap();
        fs::write(t.path().join("new"), "x").unwrap();
        set_file_mtime(
            t.path().join("old"),
            FileTime::from_system_time(now - day * 10),
        )
        .unwrap();
        set_file_mtime(
            t.path().join("new"),
            FileTime::from_system_time(now - day * 2),
        )
        .unwrap();
        set_file_mtime(t.path(), FileTime::from_system_time(now - day * 20)).unwrap();
        let newest = measure(t.path()).unwrap().newest;
        let age = now.duration_since(newest).unwrap();
        assert!(age >= day * 2 - Duration::from_secs(2) && age <= day * 2 + Duration::from_secs(2));
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
