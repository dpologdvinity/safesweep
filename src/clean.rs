//! Removing candidates, and keeping a record of what was removed.
//!
//! [`clean`] is the only code in the crate that deletes anything. It never trusts the earlier
//! scan: before each removal it measures the candidate again and runs the full
//! [`crate::guard::check`] again, with a fresh look at running processes. A candidate that was
//! safe a moment ago but is now in use, or has changed, is skipped.
//!
//! # The manifest
//!
//! Every removal is written to a log file, the *manifest*, in `~/.local/state/safesweep/`
//! (one tab-separated line per path). The log is opened before anything is deleted: if it cannot
//! be created, nothing is removed. If it stops being writable midway, the run stops after
//! reporting what it just did, so there is never an unrecorded deletion beyond the last one.

use crate::guard::{SnapshotCache, check};
use crate::render::{escape_path, escape_text, human};
use crate::scan::measure;
use crate::{Ctx, Row};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

/// The result of a [`clean`] run, for the caller to print.
pub struct Outcome {
    /// Bytes freed (or, for a dry run, that would be freed).
    pub freed: u64,
    /// How many candidates were removed.
    pub removed: usize,
    /// How many removals failed or could not be logged. A candidate skipped by the re-check is
    /// *not* a failure.
    pub failed: usize,
    /// One human-readable line per candidate (`removed ...`, `would remove ...`, `skipped ...:
    /// reason`, `failed ...: error`), plus a final `stopped: ...` line if the log broke.
    pub lines: Vec<String>,
}

/// Removes the candidates in `rows` whose status is `ok`, or reports what would be removed.
///
/// With `apply` false this is a dry run: nothing is touched and no manifest is written.
///
/// With `apply` true, each candidate is re-measured and re-checked (see the module docs), then
/// removed: a directory with all its contents, or a single file. The removal is recorded in the
/// manifest in `manifest_dir`. Candidates that fail the re-check are reported as `skipped` and
/// left alone.
///
/// # Errors
///
/// Returns an error, with nothing deleted, if the manifest directory or file cannot be created.
/// A failure to write the manifest *after* a removal is not an error: the removal is reported
/// in the returned [`Outcome`] together with a `stopped:` line, and the run ends.
pub fn clean(rows: &[Row], ctx: &Ctx, apply: bool, manifest_dir: &Path) -> io::Result<Outcome> {
    let mut out = Outcome {
        freed: 0,
        removed: 0,
        failed: 0,
        lines: Vec::new(),
    };
    let ok = rows.iter().filter(|r| r.status == "ok");
    if !apply {
        for r in ok {
            out.freed += r.bytes;
            out.lines.push(format!(
                "would remove {} {}",
                human(r.bytes),
                escape_path(&r.path)
            ));
        }
        return Ok(out);
    }
    // The manifest is the only record of what was deleted, so no manifest means no deleting.
    // The file name is the start time in seconds; two runs in the same second share one file.
    fs::create_dir_all(manifest_dir)?;
    let secs = ctx
        .now
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut manifest = OpenOptions::new()
        .create(true)
        .append(true)
        .open(manifest_dir.join(format!("{secs}.log")))?;
    // What processes have open is re-read at most once a second: re-reading it for every
    // candidate (tens of milliseconds each) made a big cleanup slow, while a run-wide reading
    // would go stale as large trees are deleted. See `SnapshotCache` for the trade-off.
    let mut snapshot = SnapshotCache::new(ctx.proc_root.clone(), Duration::from_secs(1));
    for r in ok {
        // the path as it is printed and logged: control characters escaped (see `escape_path`)
        let shown = escape_path(&r.path);
        let live = snapshot.get(); // None if /proc is unreadable: check() then refuses
        let root = r.rule.root_path(ctx);
        let fresh = measure(&r.path).map_err(|e| e.to_string()).and_then(|e| {
            check(
                &r.path,
                r.rule.busy_scope(&r.path),
                &e,
                &root,
                r.rule.min_age_days,
                ctx,
                live,
            )
            .map(|()| e)
        });
        let entry = match fresh {
            Ok(e) => e,
            Err(reason) => {
                out.lines
                    .push(format!("skipped {}: {}", shown, escape_text(&reason)));
                continue;
            }
        };
        // symlink_metadata, not metadata: a candidate that became a symlink since the check is
        // removed as a link and its target is left alone.
        let removed = match fs::symlink_metadata(&r.path) {
            Ok(md) if md.is_dir() => fs::remove_dir_all(&r.path),
            Ok(_) => fs::remove_file(&r.path),
            Err(e) => Err(e),
        };
        let logged = match &removed {
            Ok(()) => writeln!(manifest, "{}\t{}", entry.bytes, shown),
            Err(e) => writeln!(manifest, "ERROR\t{}\t{e}", shown),
        };
        match removed {
            Ok(()) => {
                out.lines
                    .push(format!("removed {} {}", human(entry.bytes), shown));
                out.freed += entry.bytes;
                out.removed += 1;
            }
            Err(e) => {
                out.lines
                    .push(format!("failed {} (may be partly removed): {e}", shown));
                out.failed += 1;
            }
        }
        // The removal already happened, so report it rather than erroring out; but with no
        // working log there is no record of further deletions, so stop here.
        if let Err(e) = logged {
            out.lines.push(format!(
                "stopped: {} not logged, manifest write failed: {e}",
                shown
            ));
            out.failed += 1;
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::RULES;
    use std::fs::{self, File};
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    struct Fx {
        _tmp: tempfile::TempDir,
        ctx: Ctx,
        manifest: PathBuf,
    }

    /// `ctx.tmp` holds candidates; the `tmp` rule (7 days) owns them.
    fn fx() -> Fx {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx {
            home: PathBuf::new(),
            tmp: tmp.path().join("t"),
            proc_root: "/proc".into(),
            euid: unsafe { libc::geteuid() },
            // ctime cannot be backdated, so age fixtures by moving the clock instead
            now: SystemTime::now() + Duration::from_secs(86400 * 10),
        };
        fs::create_dir(&ctx.tmp).unwrap();
        let manifest = tmp.path().join("state");
        Fx {
            _tmp: tmp,
            ctx,
            manifest,
        }
    }

    fn candidate(fx: &Fx, name: &str, status: &str) -> Row {
        let path = fx.ctx.tmp.join(name);
        fs::create_dir(&path).unwrap();
        fs::write(path.join("f"), vec![1u8; 4096]).unwrap();
        Row {
            category: "tmp",
            path,
            bytes: 4096,
            regen: RULES.iter().find(|r| r.id == "tmp").unwrap().regen,
            status: status.into(),
            rule: RULES.iter().find(|r| r.id == "tmp").unwrap(),
        }
    }

    fn manifest_text(dir: &Path) -> String {
        fs::read_dir(dir)
            .unwrap()
            .map(|e| fs::read_to_string(e.unwrap().path()).unwrap())
            .collect()
    }

    #[test]
    fn dry_run_deletes_nothing() {
        let fx = fx();
        let rows = [candidate(&fx, "c", "ok")];
        let out = clean(&rows, &fx.ctx, false, &fx.manifest).unwrap();
        assert!(rows[0].path.exists());
        assert_eq!(out.freed, 4096);
        assert!(!fx.manifest.exists());
    }

    #[test]
    fn apply_removes_ok_rows_only() {
        let fx = fx();
        let rows = [
            candidate(&fx, "c", "ok"),
            candidate(&fx, "k", "skipped: symlink"),
        ];
        let out = clean(&rows, &fx.ctx, true, &fx.manifest).unwrap();
        assert!(!rows[0].path.exists());
        assert!(rows[1].path.exists());
        assert_eq!((out.removed, out.failed), (1, 0));
        assert!(out.freed > 0);
        let line = format!("{}\t{}\n", out.freed, escape_path(&rows[0].path));
        assert_eq!(manifest_text(&fx.manifest), line);
    }

    #[test]
    fn apply_skips_vanished_path() {
        let fx = fx();
        let rows = [candidate(&fx, "c", "ok")];
        fs::remove_dir_all(&rows[0].path).unwrap();
        let out = clean(&rows, &fx.ctx, true, &fx.manifest).unwrap();
        assert_eq!(out.failed, 0);
        assert!(out.lines[0].starts_with("skipped"), "{:?}", out.lines);
    }

    #[test]
    fn apply_rechecks_open_files() {
        let fx = fx();
        let rows = [candidate(&fx, "c", "ok")];
        let _held = File::open(rows[0].path.join("f")).unwrap();
        let out = clean(&rows, &fx.ctx, true, &fx.manifest).unwrap();
        assert!(rows[0].path.exists());
        assert!(out.lines[0].starts_with("skipped"), "{:?}", out.lines);
    }

    #[test]
    fn apply_aborts_without_manifest() {
        let fx = fx();
        let rows = [candidate(&fx, "c", "ok")];
        fs::write(&fx.manifest, "not a dir").unwrap();
        assert!(clean(&rows, &fx.ctx, true, &fx.manifest).is_err());
        assert!(rows[0].path.exists());
    }

    #[test]
    fn manifest_write_failure_after_removal_is_reported_not_hidden() {
        // disk full after the log was opened: the removal happened, so say so, and stop
        let fx = fx();
        let rows = [candidate(&fx, "a", "ok"), candidate(&fx, "b", "ok")];
        fs::create_dir_all(&fx.manifest).unwrap();
        let secs = fx.ctx.now.duration_since(UNIX_EPOCH).unwrap().as_secs();
        std::os::unix::fs::symlink("/dev/full", fx.manifest.join(format!("{secs}.log"))).unwrap();
        let out = clean(&rows, &fx.ctx, true, &fx.manifest).unwrap();
        assert_eq!(out.removed, 1);
        assert!(
            !rows[0].path.exists() && rows[1].path.exists(),
            "stops after the log breaks"
        );
        assert!(out.failed >= 1);
        assert!(
            out.lines.iter().any(|l| l.contains("not logged")),
            "{:?}",
            out.lines
        );
    }
}
