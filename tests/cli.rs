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

/// What to do when a test needs `unshare -rm` (unprivileged user and mount namespaces):
/// `Ok(true)` run it, `Ok(false)` skip it, `Err(message)` fail the test.
///
/// A skip is silent in test output, and these tests guard the most important safety checks
/// (files used inside containers, mount points). So on CI, where a skip would hide that a
/// regression test never ran, being unable to use namespaces is a failure, not a skip.
fn namespace_policy(available: bool, on_ci: bool) -> Result<bool, String> {
    match (available, on_ci) {
        (true, _) => Ok(true),
        (false, false) => Ok(false),
        (false, true) => Err(
            "`unshare -rm` is unavailable on CI, so the mount-namespace safety \
                              tests cannot run; enable unprivileged user namespaces on the runner"
                .into(),
        ),
    }
}

/// Whether this test can run `unshare -rm`; panics on CI if it cannot. See [`namespace_policy`].
fn namespaces_usable() -> bool {
    let works = std::process::Command::new("unshare")
        .args(["-rm", "true"])
        .status()
        .is_ok_and(|s| s.success());
    match namespace_policy(works, std::env::var_os("CI").is_some()) {
        Ok(run) => {
            if !run {
                eprintln!("skipping: unshare -rm unavailable");
            }
            run
        }
        Err(msg) => panic!("{msg}"),
    }
}

#[test]
fn namespace_policy_skips_locally_and_fails_on_ci() {
    assert_eq!(namespace_policy(true, false), Ok(true));
    assert_eq!(namespace_policy(true, true), Ok(true));
    assert_eq!(namespace_policy(false, false), Ok(false));
    let err = namespace_policy(false, true).unwrap_err();
    assert!(err.contains("unshare") && err.contains("CI"), "{err}");
}

/// Runs the binary inside a user+mount namespace with `setup` mounts applied first.
/// Returns None if this system does not allow unprivileged namespaces.
fn scan_in_namespace(fx: &Fx, setup: &str) -> Option<BTreeMap<String, String>> {
    if !namespaces_usable() {
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

#[test]
fn why_reports_cpu() {
    let fx = Fx::new();
    let out = fx
        .cmd()
        .args(["why", "--interval-ms", "200"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    assert!(out.starts_with("CPU busy: "), "{out}");
    assert!(out.contains("PROGRAM"), "{out}");
}

#[test]
fn artifacts_lists_idle_projects_but_guard_still_applies() {
    let fx = Fx::new();
    let h = fx.home.path();
    let old = FileTime::from_system_time(SystemTime::now() - Duration::from_secs(86400 * 60));
    for (name, git_age) in [("idle", Some(old)), ("active", None)] {
        put(h.join(format!("code/{name}/node_modules/x/f")));
        put(h.join(format!("code/{name}/node_modules/.package-lock.json")));
        put(h.join(format!("code/{name}/package.json")));
        fs::create_dir_all(h.join(format!("code/{name}/.git/logs"))).unwrap();
        for f in ["logs/HEAD", "index"] {
            let p = h.join(format!("code/{name}/.git/{f}"));
            fs::write(&p, "x").unwrap();
            if let Some(t) = git_age {
                set_file_mtime(&p, t).unwrap();
            }
        }
    }
    // opt-in: absent from the default scan
    assert!(!fx.statuses(&[]).keys().any(|k| k.contains("node_modules")));
    // listed for the idle project only, and still skipped: its tree changed just now
    let want = BTreeMap::from([(
        "code/idle/node_modules".into(),
        "skipped: modified within 30 days".into(),
    )]);
    assert_eq!(fx.statuses(&["--category", "artifacts"]), want);
}

#[test]
fn why_rejects_tiny_interval() {
    let fx = Fx::new();
    let out = fx
        .cmd()
        .args(["why", "--interval-ms", "0"])
        .assert()
        .code(2)
        .get_output()
        .stderr
        .clone();
    assert!(String::from_utf8_lossy(&out).contains("--interval-ms must be at least 100"));
}

#[test]
fn why_survives_closed_pipe() {
    let fx = Fx::new();
    let bin = assert_cmd::cargo::cargo_bin("safesweep");
    let mut child = std::process::Command::new(bin)
        .args(["why", "--interval-ms", "200"])
        .env("HOME", fx.home.path())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take()); // like `| head -0`: the reader is gone before output arrives
    let out = child.wait_with_output().unwrap();
    assert!(!String::from_utf8_lossy(&out.stderr).contains("panicked"));
}

#[test]
fn tui_needs_a_terminal() {
    let fx = Fx::new();
    let out = fx
        .cmd()
        .arg("tui")
        .assert()
        .code(2)
        .get_output()
        .stderr
        .clone();
    assert!(String::from_utf8_lossy(&out).contains("error: tui needs an interactive terminal"));
}

#[test]
fn in_use_seen_through_other_mount_namespace() {
    // podman / unshare -m: the process sees the candidate under another path (/mnt here),
    // so path text in /proc never matches; only the inode does.
    let fx = Fx::new();
    let live = fx.home.path().join("proj/__pycache__");
    let mnt = fx.home.path().join("mnt");
    fs::create_dir(&mnt).unwrap();
    if !namespaces_usable() {
        return;
    }
    let mut child = std::process::Command::new("unshare")
        .args([
            "-rm",
            "sh",
            "-c",
            r#"mount --bind "$0" "$1" && cd "$1" && echo ready && exec sleep 30"#,
        ])
        .arg(&live)
        .arg(&mnt)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    std::io::BufRead::read_line(
        &mut std::io::BufReader::new(child.stdout.take().unwrap()),
        &mut line,
    )
    .unwrap();
    assert_eq!(line.trim(), "ready");
    let got = fx.statuses(&["--category", "pycache"]);
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(got["proj/__pycache__"], "skipped: in use by a process");
}

#[test]
fn relative_home_exits_2() {
    let fx = Fx::new();
    let out = fx
        .cmd()
        .env("HOME", "rel")
        .assert()
        .code(2)
        .get_output()
        .stderr
        .clone();
    assert!(String::from_utf8_lossy(&out).contains("HOME must be an absolute path"));
}

#[test]
fn category_with_why_is_an_error() {
    let fx = Fx::new();
    let out = fx
        .cmd()
        .args(["--category", "pip", "why"])
        .assert()
        .code(2)
        .get_output()
        .stderr
        .clone();
    assert!(String::from_utf8_lossy(&out).contains("--category does not apply to why"));
}

#[test]
fn completions_for_each_shell() {
    let fx = Fx::new();
    for (shell, marker) in [
        ("bash", "complete -F"),
        ("zsh", "#compdef safesweep"),
        ("fish", "complete -c safesweep"),
    ] {
        let out = fx
            .cmd()
            .args(["completions", shell])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let out = String::from_utf8_lossy(&out);
        assert!(out.contains(marker), "{shell}: {out}");
        assert!(
            out.contains("artifacts") || out.contains("clean"),
            "{shell} lists subcommands"
        );
    }
    fx.cmd()
        .args(["completions", "nosuchshell"])
        .assert()
        .code(2);
}

#[test]
fn man_page_is_roff() {
    let fx = Fx::new();
    let out = fx
        .cmd()
        .arg("man")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    assert!(
        out.starts_with(".ie") || out.starts_with(".TH"),
        "{}",
        &out[..out.len().min(80)]
    );
    assert!(
        out.contains("safesweep") && out.contains("clean"),
        "mentions the command and subcommands"
    );
}

#[test]
fn category_with_completions_and_man_is_an_error() {
    let fx = Fx::new();
    for args in [
        ["--category", "pip", "completions", "bash"].as_slice(),
        ["--category", "pip", "man"].as_slice(),
    ] {
        let out = fx
            .cmd()
            .args(args)
            .assert()
            .code(2)
            .get_output()
            .stderr
            .clone();
        assert!(
            String::from_utf8_lossy(&out).contains("--category does not apply to"),
            "{args:?}"
        );
    }
}
