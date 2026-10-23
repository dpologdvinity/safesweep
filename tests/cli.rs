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
        put(h.join(".cache/old/f"));
        put(h.join(".cache/fresh/f"));
        let held = put(h.join(".cache/held/f"));
        age(&h.join(".cache/old"), 10);
        age(&h.join(".cache/held"), 10);
        put(h.join("proj/__pycache__/a.pyc"));
        put(tmp.path().join("stale/f"));
        age(&tmp.path().join("stale"), 10);
        let _held = File::open(held).unwrap();
        Fx { home, tmp, _held }
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
    let out = fx
        .cmd()
        .args(["scan", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&out).unwrap();
    let got: BTreeMap<String, String> = rows
        .iter()
        .map(|r| {
            (
                fx.rel(r["path"].as_str().unwrap()),
                r["status"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let want = BTreeMap::from([
        (".cache/old".into(), "ok".into()),
        (
            ".cache/fresh".into(),
            "skipped: modified within 7 days".into(),
        ),
        (".cache/held".into(), "skipped: in use by a process".into()),
        ("stale".into(), "ok".into()),
    ]);
    assert_eq!(got, want);
}

#[test]
fn pycache_is_opt_in() {
    let fx = Fx::new();
    let out = fx
        .cmd()
        .args(["--category", "pycache", "scan", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&out).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        fx.rel(rows[0]["path"].as_str().unwrap()),
        "proj/__pycache__"
    );
    assert_eq!(rows[0]["status"], "ok");
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

    fx.cmd().args(["clean", "--apply"]).assert().success();
    assert!(!h.join(".cache/old").exists());
    assert!(!t.join("stale").exists());
    assert!(h.join(".cache/fresh").exists() && h.join(".cache/held").exists());
    // opt-in rule: untouched by a default clean
    assert!(h.join("proj/__pycache__").exists());

    let logs: Vec<_> = fs::read_dir(h.join(".local/state/safesweep"))
        .unwrap()
        .collect();
    assert_eq!(logs.len(), 1);
    let log = fs::read_to_string(logs[0].as_ref().unwrap().path()).unwrap();
    let mut removed: Vec<String> = log
        .lines()
        .map(|l| fx.rel(l.split('\t').nth(1).unwrap()))
        .collect();
    removed.sort();
    assert_eq!(removed, [".cache/old", "stale"]);
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
