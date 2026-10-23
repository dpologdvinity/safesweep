use crate::guard::{check, in_use};
use crate::render::{escape_path, human};
use crate::scan::measure;
use crate::{Ctx, Row};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::time::UNIX_EPOCH;

pub struct Outcome {
    pub freed: u64,
    pub removed: usize,
    pub failed: usize,
    pub lines: Vec<String>,
}

/// Remove the `ok` rows (or report what would go, if `!apply`), re-checking each one first.
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
    for r in ok {
        // Fresh /proc snapshot per candidate: a run-wide one goes stale while big trees are deleted.
        let live = in_use(&ctx.proc_root).ok();
        let root = r.rule.root_path(ctx);
        let fresh = measure(&r.path).map_err(|e| e.to_string()).and_then(|e| {
            check(
                &r.path,
                r.rule.busy_scope(&r.path),
                &e,
                &root,
                r.rule.min_age_days,
                ctx,
                live.as_deref(),
            )
            .map(|()| e)
        });
        let entry = match fresh {
            Ok(e) => e,
            Err(reason) => {
                out.lines
                    .push(format!("skipped {}: {reason}", escape_path(&r.path)));
                continue;
            }
        };
        let removed = match fs::symlink_metadata(&r.path) {
            Ok(md) if md.is_dir() => fs::remove_dir_all(&r.path),
            Ok(_) => fs::remove_file(&r.path),
            Err(e) => Err(e),
        };
        match removed {
            Ok(()) => {
                writeln!(manifest, "{}\t{}", entry.bytes, escape_path(&r.path))?;
                out.lines.push(format!(
                    "removed {} {}",
                    human(entry.bytes),
                    escape_path(&r.path)
                ));
                out.freed += entry.bytes;
                out.removed += 1;
            }
            Err(e) => {
                writeln!(manifest, "ERROR\t{}\t{e}", escape_path(&r.path))?;
                out.lines.push(format!(
                    "failed {} (may be partly removed): {e}",
                    escape_path(&r.path)
                ));
                out.failed += 1;
            }
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
}
