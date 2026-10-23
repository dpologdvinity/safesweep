use rayon::prelude::*;
use rules::{Regen, Rule};
use serde::{Serialize, Serializer};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub mod clean;
pub mod guard;
pub mod render;
pub mod rules;
pub mod scan;
pub mod tui;
pub mod why;
pub mod wsl;

/// Everything that depends on the environment, so tests can supply their own.
pub struct Ctx {
    pub home: PathBuf,
    pub tmp: PathBuf,
    pub proc_root: PathBuf,
    pub euid: u32,
    pub now: SystemTime,
}

/// As root, every path under HOME is trusted, so HOME itself must belong to root:
/// `sudo -E` keeps the invoking user's HOME, and that user could point `~/.cache` anywhere.
pub fn check_root_home(euid: u32, home: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    if euid != 0 {
        return Ok(());
    }
    match std::fs::metadata(home) {
        Ok(md) if md.uid() == 0 => Ok(()),
        _ => Err(format!(
            "refusing to run as root with HOME={} not owned by root (use `sudo -H`, or run as that user)",
            home.display()
        )),
    }
}

impl Ctx {
    pub fn from_env() -> Result<Ctx, String> {
        // An empty HOME would make `Root::Home("")` the current directory.
        let home = std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .ok_or("HOME is not set")?;
        let home = PathBuf::from(home);
        let euid = unsafe { libc::geteuid() };
        check_root_home(euid, &home)?;
        // TMPDIR picks the tmp rule's root, so `.`, `/` or $HOME would sweep real data.
        let tmp = std::env::temp_dir();
        let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        if !tmp.is_absolute() || real(&tmp) == Path::new("/") || real(&home).starts_with(real(&tmp))
        {
            return Err(
                "TMPDIR must be an absolute path that is not / and does not contain HOME".into(),
            );
        }
        Ok(Ctx {
            home,
            tmp,
            proc_root: "/proc".into(),
            euid,
            now: SystemTime::now(),
        })
    }
}

/// One candidate as shown by `scan` and acted on by `clean`.
#[derive(Serialize, Clone)]
pub struct Row {
    pub category: &'static str,
    #[serde(serialize_with = "lossy")]
    pub path: PathBuf,
    pub bytes: u64,
    pub regen: Regen,
    /// `"ok"` or `"skipped: <reason>"`.
    pub status: String,
    #[serde(skip)]
    pub rule: &'static Rule,
}

// serde_json rejects non-UTF-8 paths, and a file name must never break the output.
fn lossy<S: Serializer>(path: &Path, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&render::escape_path(path))
}

/// Scan and guard every candidate of `rules`, largest first.
pub fn collect(rules: &[&'static Rule], ctx: &Ctx) -> Vec<Row> {
    let live = guard::in_use(&ctx.proc_root).ok();
    let mounts = scan::mount_points();
    let rows: Vec<Row> = rules
        .par_iter()
        .flat_map_iter(|&rule| {
            let root = rule.root_path(ctx);
            scan::find(&root, &rule.matcher)
                .into_iter()
                .map(|path| {
                    let measured = match &mounts {
                        Ok(m) => scan::measure_with(&path, m),
                        Err(e) => Err(std::io::Error::new(e.kind(), e.to_string())),
                    };
                    let (bytes, status) = match measured {
                        Err(e) => (0, format!("skipped: {e}")),
                        Ok(e) => {
                            let verdict = guard::check(
                                &path,
                                rule.busy_scope(&path),
                                &e,
                                &root,
                                rule.min_age_days,
                                ctx,
                                live.as_ref(),
                            );
                            (
                                e.bytes,
                                verdict.map_or_else(|r| format!("skipped: {r}"), |()| "ok".into()),
                            )
                        }
                    };
                    Row {
                        category: rule.id,
                        path,
                        bytes,
                        regen: rule.regen,
                        status,
                        rule,
                    }
                })
                .collect::<Vec<_>>()
        })
        .collect();
    // Rules overlap (a __pycache__ inside a ~/.cache child, or ~/.cache symlinked into the temp
    // dir): compare real locations and keep only the outermost path. The parent is resolved,
    // not the candidate itself, so a candidate symlink is never followed.
    let real = |p: &Path| match (p.parent().map(std::fs::canonicalize), p.file_name()) {
        (Some(Ok(parent)), Some(name)) => parent.join(name),
        _ => p.to_path_buf(),
    };
    let mut keyed: Vec<(PathBuf, Row)> = rows.into_iter().map(|r| (real(&r.path), r)).collect();
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    let mut kept: Vec<(PathBuf, Row)> = Vec::with_capacity(keyed.len());
    for (key, row) in keyed {
        if !kept.last().is_some_and(|(k, _)| key.starts_with(k)) {
            kept.push((key, row));
        }
    }
    let mut kept: Vec<Row> = kept.into_iter().map(|(_, r)| r).collect();
    kept.sort_by_key(|r| std::cmp::Reverse(r.bytes));
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use filetime::{FileTime, set_file_mtime};
    use std::collections::BTreeSet;
    use std::fs;
    use std::time::Duration;

    fn age(path: &Path, days: u32) {
        let when =
            FileTime::from_system_time(SystemTime::now() - Duration::from_secs(86400) * days);
        for e in walkdir::WalkDir::new(path).contents_first(true) {
            set_file_mtime(e.unwrap().path(), when).unwrap();
        }
    }

    fn ctx(home: &Path, tmp: &Path) -> Ctx {
        Ctx {
            home: home.into(),
            tmp: tmp.into(),
            proc_root: "/proc".into(),
            euid: unsafe { libc::geteuid() },
            // ctime cannot be backdated, so age fixtures by moving the clock instead
            now: SystemTime::now() + Duration::from_secs(86400 * 10),
        }
    }

    fn summary(rows: &[Row], home: &Path) -> BTreeSet<(String, String, String)> {
        rows.iter()
            .map(|r| {
                let rel = r
                    .path
                    .strip_prefix(home)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                (r.category.to_string(), rel, r.status.clone())
            })
            .collect()
    }

    #[test]
    fn collect_fixture_home() {
        let (home, tmp) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let h = home.path();
        fs::create_dir_all(h.join(".cache/old")).unwrap();
        fs::write(h.join(".cache/old/f"), "x").unwrap();
        fs::create_dir_all(h.join(".cache/pip")).unwrap();
        fs::write(h.join(".cache/pip/x"), "x").unwrap();
        age(&h.join(".cache"), 10);
        fs::create_dir_all(h.join("p/__pycache__")).unwrap();
        fs::write(h.join("p/__pycache__/a.pyc"), "x").unwrap();

        let all: Vec<_> = rules::RULES.iter().collect();
        let rows = collect(&all, &ctx(h, tmp.path()));
        let ok = |c: &str, p: &str| (c.to_string(), p.to_string(), "ok".to_string());
        assert_eq!(
            summary(&rows, h),
            BTreeSet::from([
                ok("xdg-cache", ".cache/old"),
                ok("pip", ".cache/pip"),
                ok("pycache", "p/__pycache__"),
            ])
        );
    }

    #[test]
    fn collect_dedupes_nested() {
        let (home, tmp) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let h = home.path();
        fs::create_dir_all(h.join(".cache/old/__pycache__")).unwrap();
        fs::write(h.join(".cache/old/__pycache__/a.pyc"), "x").unwrap();
        age(&h.join(".cache"), 10);

        let all: Vec<_> = rules::RULES.iter().collect();
        let rows = collect(&all, &ctx(h, tmp.path()));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].category, "xdg-cache");
    }

    #[test]
    fn collect_dedupes_through_symlinked_root() {
        // ~/.cache is a symlink into the temp dir: the same directory is reachable by two rules
        let (home, tmp) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let real = tmp.path().join("real");
        fs::create_dir_all(real.join("x")).unwrap();
        fs::write(real.join("x/f"), vec![1u8; 4096]).unwrap();
        std::os::unix::fs::symlink(&real, home.path().join(".cache")).unwrap();

        let all: Vec<_> = rules::RULES.iter().filter(|r| !r.opt_in).collect();
        let rows = collect(&all, &ctx(home.path(), tmp.path()));
        assert_eq!(
            rows.len(),
            1,
            "{:?}",
            rows.iter().map(|r| &r.path).collect::<Vec<_>>()
        );
    }

    #[test]
    fn root_needs_root_owned_home() {
        let t = tempfile::tempdir().unwrap(); // owned by the test user, never root in CI
        let me = unsafe { libc::geteuid() };
        if me == 0 {
            return;
        }
        assert_eq!(check_root_home(me, t.path()), Ok(()));
        let err = check_root_home(0, t.path()).unwrap_err();
        assert!(err.contains("HOME") && err.contains("root"), "{err}");
    }
}
