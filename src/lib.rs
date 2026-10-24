//! `safesweep`: clean caches and temporary files that are provably safe to delete.
//!
//! This crate is the library behind the `safesweep` command-line tool. Its main entry points
//! are [`collect`], which finds and judges every candidate, and [`clean::clean`], which removes
//! the ones judged safe. The binary (`src/main.rs`) is a thin command-line wrapper around them.
//!
//! # How a run works
//!
//! ```text
//!  rules ──► scan::find ──► scan::measure ──► guard::check ──► Row ──► render / tui / clean
//!  (what)    (candidates)    (what is in it)   (safe? why not?)  (one per candidate)
//! ```
//!
//! 1. [`rules::RULES`] lists the locations the tool knows how to clean.
//! 2. [`scan`] finds the candidate paths under each rule and measures them. It only observes.
//! 3. [`guard`] turns each measurement into a verdict: `ok`, or the reason to skip.
//! 4. [`collect`] bundles the result as one [`Row`] per candidate, largest first.
//! 5. [`render`] prints the rows, [`tui`] lets the user pick from them, and [`clean`] removes the
//!    chosen ones, re-checking each one first.
//!
//! The separate [`why`] module reports what is using the CPU and shares no state with the rest.
//!
//! See `ARCHITECTURE.md` in the repository for the design in more detail.

#![warn(missing_docs)]

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

/// Everything about the environment that the tool depends on, gathered in one place.
///
/// Code takes a `Ctx` instead of reading the environment itself, so tests can supply a fake
/// home directory, a fake clock and a fake `/proc`, and so the real values are validated once,
/// in [`Ctx::from_env`].
#[derive(Clone)]
pub struct Ctx {
    /// The user's home directory. Always an absolute path.
    pub home: PathBuf,
    /// The temporary directory the `tmp` rule cleans (`$TMPDIR`, or `/tmp`).
    pub tmp: PathBuf,
    /// Where the process information is read from. Always `/proc` outside tests.
    pub proc_root: PathBuf,
    /// The effective user id of this process. Zero means running as root.
    pub euid: u32,
    /// "Now", used to decide how old files are. Tests move it forward to age files that cannot
    /// be back-dated.
    pub now: SystemTime,
}

/// Refuses to run as root with a home directory owned by someone else.
///
/// Root skips the ownership check, so every path under `home` would be trusted. `sudo -E` keeps
/// the invoking user's `HOME`, and that user could point `~/.cache` at a system directory and
/// have root delete its contents. Requiring `home` to belong to root closes that hole.
///
/// Does nothing when `euid` is not 0.
///
/// # Errors
///
/// Returns an explanation (mentioning `sudo -H`) if running as root and `home` is not owned by
/// root or cannot be inspected.
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
    /// Builds the context from the real environment and refuses anything unsafe.
    ///
    /// # Errors
    ///
    /// Returns a message (printed to the user, exit code 2) if:
    /// - `HOME` is unset, empty or relative: rules are relative to it, and an empty or relative
    ///   value would make them act on the current directory;
    /// - running as root with a `HOME` not owned by root (see [`check_root_home`]);
    /// - `TMPDIR` is relative, is `/`, or contains `HOME`: the `tmp` rule would then sweep
    ///   real data.
    pub fn from_env() -> Result<Ctx, String> {
        // An empty HOME would make `Root::Home("")` the current directory.
        let home = std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .ok_or("HOME is not set")?;
        let home = PathBuf::from(home);
        // a relative HOME would make every rule act on paths under the current directory
        if !home.is_absolute() {
            return Err("HOME must be an absolute path".into());
        }
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

/// One candidate: the unit that `scan` lists, the TUI shows and `clean` acts on.
///
/// This is also the shape of the `scan --json` output.
#[derive(Serialize, Clone)]
pub struct Row {
    /// The id of the rule that found it (`pip`, `tmp`, ...).
    pub category: &'static str,
    /// The candidate path. Serialized with unprintable characters escaped (see [`render::escape_path`]).
    #[serde(serialize_with = "lossy")]
    pub path: PathBuf,
    /// Bytes that deleting it would free (0 if it could not be measured).
    pub bytes: u64,
    /// What it costs to get the data back.
    pub regen: Regen,
    /// `"ok"` if it passed the guard, otherwise `"skipped: <reason>"`.
    pub status: String,
    /// The rule that found it, which `clean` needs to re-check the candidate. Not serialized.
    #[serde(skip)]
    pub rule: &'static Rule,
}

/// Serializes a path as an escaped string. `serde_json` rejects non-UTF-8 paths, and a file name
/// must never be able to break the output or fake extra lines.
fn lossy<S: Serializer>(path: &Path, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&render::escape_path(path))
}

/// Finds every candidate of `rules`, judges each one, and returns them largest first.
///
/// Rules are scanned in parallel. Every candidate yields a [`Row`], never an error: one that
/// cannot be measured or fails the guard becomes a row with a `skipped: <reason>` status, so the
/// user can see why. The returned list is free of overlaps: if one candidate lies inside
/// another found by a different rule (a `__pycache__` inside a `~/.cache` entry), only the
/// outer one is kept, so nothing is counted or deleted twice.
///
/// `/proc` and the mount table are read once for the whole scan. If either cannot be read, every
/// candidate is reported as unverifiable instead.
pub fn collect(rules: &[&'static Rule], ctx: &Ctx) -> Vec<Row> {
    let live = guard::in_use(&ctx.proc_root).ok();
    let mounts = scan::mount_points();
    let rows: Vec<Row> = rules
        .par_iter()
        .flat_map_iter(|&rule| {
            let root = rule.root_path(ctx);
            // Candidates are judged in parallel too: one rule can find thousands (every
            // `__pycache__` in the home directory), and judging each costs several system calls.
            scan::find(&root, &rule.matcher)
                .into_par_iter()
                .map(|path| {
                    // io::Error is not Clone, so rebuild it to report the same failure per candidate
                    let measured = match &mounts {
                        Ok(m) => scan::measure_with(&path, m),
                        Err(e) => Err(std::io::Error::new(e.kind(), e.to_string())),
                    };
                    let (bytes, status) = match measured {
                        Err(e) => (
                            0,
                            format!("skipped: {}", render::escape_text(&e.to_string())),
                        ),
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
                                verdict.map_or_else(
                                    |r| format!("skipped: {}", render::escape_text(&r)),
                                    |()| "ok".into(),
                                ),
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

    #[test]
    fn skip_reasons_from_io_errors_are_escaped() {
        // another user can name an unreadable subdir so the error text fakes a table row
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        use std::os::unix::fs::PermissionsExt;
        let (home, tmp) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let bad = tmp.path().join("c/x\nok 9.9 GiB fake");
        fs::create_dir_all(&bad).unwrap();
        fs::set_permissions(&bad, fs::Permissions::from_mode(0o300)).unwrap();
        let tmp_rule: Vec<_> = rules::RULES.iter().filter(|r| r.id == "tmp").collect();
        let rows = collect(&tmp_rule, &ctx(home.path(), tmp.path()));
        fs::set_permissions(&bad, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].status.starts_with("skipped"), "{}", rows[0].status);
        assert!(!rows[0].status.contains('\n'), "{:?}", rows[0].status);
    }
}
