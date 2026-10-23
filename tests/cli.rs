use assert_cmd::Command;
use filetime::{FileTime, set_file_mtime};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tempfile::TempDir;

fn age(path: &Path, days: u32) {
    let when = FileTime::from_system_time(SystemTime::now() - Duration::from_secs(86400) * days);
    for e in walkdir::WalkDir::new(path).contents_first(true) {
        set_file_mtime(e.unwrap().path(), when).unwrap();
    }
}

fn put(path: PathBuf) -> PathBuf {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, vec![1u8; 4096]).unwrap();
    path
}

struct Fx {
    home: TempDir,
    tmp: TempDir,
    _held: File,
}

impl Fx {
    fn new() -> Fx {
        let (home, tmp) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let h = home.path();
        // The binary runs on the real clock and ctime cannot be backdated, so every
        // age-gated candidate is recent; deletions go through the 0-day pycache rule.
        put(h.join(".cache/old/f"));
        put(h.join(".cache/fresh/f"));
        age(&h.join(".cache/old"), 10); // backdated mtime, like tar/cp -p
        put(h.join("proj/__pycache__/a.pyc"));
        let held = put(h.join("busy/__pycache__/b.pyc"));
        put(tmp.path().join("stale/f"));
        age(&tmp.path().join("stale"), 10);
        let _held = File::open(held).unwrap();
        Fx { home, tmp, _held }
    }

    /// `relative path -> status` from `scan --json` with extra args.
    fn statuses(&self, args: &[&str]) -> BTreeMap<String, String> {
        let out = self
            .cmd()
            .args(args)
            .args(["scan", "--json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let rows: Vec<serde_json::Value> = serde_json::from_slice(&out).unwrap();
        rows.iter()
            .map(|r| {
                let path = self.rel(r["path"].as_str().unwrap());
                (path, r["status"].as_str().unwrap().to_string())
            })
            .collect()
    }

    fn cmd(&self) -> Command {
        let mut c = Command::cargo_bin("safesweep").unwrap();
        c.env("HOME", self.home.path())
            .env("TMPDIR", self.tmp.path());
        c
    }

    /// Path relative to the fixture home or tmp, for readable assertions.
    fn rel(&self, p: &str) -> String {
        let p = Path::new(p);
        let base = if p.starts_with(self.home.path()) {
            self.home.path()
        } else {
            self.tmp.path()
        };
        p.strip_prefix(base).unwrap().to_string_lossy().into_owned()
    }
}

#[test]
fn scan_json_statuses() {
    let fx = Fx::new();
    let recent = "skipped: modified within 7 days".to_string();
    let want = BTreeMap::from([
        (".cache/old".into(), recent.clone()), // backdated mtime, fresh ctime
        (".cache/fresh".into(), recent.clone()),
        ("stale".into(), recent),
    ]);
    assert_eq!(fx.statuses(&[]), want);
}

#[test]
fn pycache_is_opt_in() {
    let fx = Fx::new();
    let want = BTreeMap::from([
        ("proj/__pycache__".into(), "ok".into()),
        (
            "busy/__pycache__".into(),
            "skipped: in use by a process".into(),
        ),
    ]);
    assert_eq!(fx.statuses(&["--category", "pycache"]), want);
}

#[test]
fn rejects_unsafe_tmpdir() {
    let fx = Fx::new();
    for bad in [Path::new("."), fx.home.path(), Path::new("/")] {
        let out = fx
            .cmd()
            .env("TMPDIR", bad)
            .assert()
            .code(2)
            .get_output()
            .stderr
            .clone();
        assert!(
            String::from_utf8_lossy(&out).contains("error: TMPDIR must be"),
            "TMPDIR={bad:?}"
        );
    }
}

/// Runs the binary inside a user+mount namespace with `setup` mounts applied first.
/// Returns None if this system does not allow unprivileged namespaces.
fn scan_in_namespace(fx: &Fx, setup: &str) -> Option<BTreeMap<String, String>> {
    let ok = std::process::Command::new("unshare")
        .args(["-rm", "true"])
        .status();
    if !ok.is_ok_and(|s| s.success()) {
        eprintln!("skipping: unshare -rm unavailable");
        return None;
    }
    let bin = assert_cmd::cargo::cargo_bin("safesweep");
    let script = format!("{setup} && exec \"$0\" --category pycache scan --json");
    let out = std::process::Command::new("unshare")
        .args(["-rm", "sh", "-c", &script])
        .arg(bin)
        .env("HOME", fx.home.path())
        .env("TMPDIR", fx.tmp.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).unwrap();
    Some(
        rows.iter()
            .map(|r| {
                (
                    fx.rel(r["path"].as_str().unwrap()),
                    r["status"].as_str().unwrap().to_string(),
                )
            })
            .collect(),
    )
}

#[test]
fn skips_candidate_that_is_a_mount_point() {
    let fx = Fx::new();
    let dir = fx.home.path().join("proj/__pycache__");
    let setup = format!("mount -t tmpfs none '{}'", dir.display());
    if let Some(got) = scan_in_namespace(&fx, &setup) {
        assert_eq!(got["proj/__pycache__"], "skipped: contains a mount point");
    }
}

#[test]
fn skips_candidate_containing_bind_mount() {
    let fx = Fx::new();
    let precious = put(fx.home.path().join("precious/data"));
    let target = fx.home.path().join("proj/__pycache__/b");
    fs::create_dir(&target).unwrap();
    let setup = format!(
        "mount --bind '{}' '{}'",
        precious.parent().unwrap().display(),
        target.display()
    );
    if let Some(got) = scan_in_namespace(&fx, &setup) {
        assert_eq!(got["proj/__pycache__"], "skipped: contains a mount point");
    }
    assert!(precious.exists());
}

#[test]
fn clean_dry_run_then_apply() {
    let fx = Fx::new();
    let (h, t) = (fx.home.path(), fx.tmp.path());
    let dry = fx
        .cmd()
        .arg("clean")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(String::from_utf8_lossy(&dry).contains("would free"));
    assert!(h.join(".cache/old").exists() && t.join("stale").exists());

    // default rules: everything is recent (fresh ctime), and pycache is opt-in
    fx.cmd().args(["clean", "--apply"]).assert().success();
    assert!(h.join(".cache/old").exists() && t.join("stale").exists());
    assert!(h.join("proj/__pycache__").exists());

    fx.cmd()
        .args(["--category", "pycache", "clean", "--apply"])
        .assert()
        .success();
    assert!(!h.join("proj/__pycache__").exists());
    assert!(h.join("busy/__pycache__").exists());

    let log: String = fs::read_dir(h.join(".local/state/safesweep"))
        .unwrap()
        .map(|e| fs::read_to_string(e.unwrap().path()).unwrap())
        .collect();
    let removed: Vec<String> = log
        .lines()
        .map(|l| fx.rel(l.split('\t').nth(1).unwrap()))
        .collect();
    assert_eq!(removed, ["proj/__pycache__"]);
}

#[test]
fn unknown_category_exits_2() {
    let fx = Fx::new();
    let out = fx
        .cmd()
        .args(["--category", "nope"])
        .assert()
        .code(2)
        .get_output()
        .stderr
        .clone();
    assert!(String::from_utf8_lossy(&out).contains("unknown category 'nope'"));
}

#[test]
fn missing_home_exits_2() {
    let fx = Fx::new();
    let out = fx
        .cmd()
        .env_remove("HOME")
        .assert()
        .code(2)
        .get_output()
        .stderr
        .clone();
    assert!(String::from_utf8_lossy(&out).contains("error: HOME is not set"));
}
