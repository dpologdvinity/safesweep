# safesweep Stage 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `safesweep scan` and `safesweep clean [--apply]`: rule-based cache/tmp cleanup that never deletes anything in use, with a WSL compaction hint.

**Architecture:** Library crate plus thin binary. `scan` finds and measures candidates under a root (rule-agnostic), `guard` decides safe/skip, `rules` holds the static table, `lib::collect` joins them into rows, `clean` re-guards and deletes, `render` prints, `wsl` builds the hint. Rules are scanned in parallel with rayon.

**Tech Stack:** Rust 2024 edition (rustc 1.99), clap 4 (derive), walkdir 2, rayon 1, serde 1 + serde_json 1, libc 0.2. Dev: tempfile 3, filetime 0.2, assert_cmd 2.

**Spec:** `docs/agents/specs/2026-10-23-safesweep-design.md`

## Global Constraints

- Shell: cargo is not on the non-interactive PATH. Prefix commands with `. ~/.cargo/env &&`.
- Linux only. Read process info from `/proc`; never shell out to `lsof`.
- Never follow symlinks; never cross filesystem boundaries (`walkdir` `follow_links(false)`, `same_file_system(true)`).
- Size = sum of `blocks * 512` over regular files with `nlink == 1`.
- Every deletion is preceded by a fresh `measure` + `check` in the same process run.
- If the manifest file cannot be created, `clean --apply` deletes nothing and exits 1.
- Exit codes: 0 success, 1 partial failure during `--apply`, 2 bad usage (including unknown `--category` and unset `HOME`).
- Commits: Conventional Commits, small. Never stage `docs/superpowers/`, `.claude/`, `CLAUDE.md` (already in `.git/info/exclude`).
- Every task ends with `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` all passing.

## Review Focus

1. `HOME` unset or empty: must exit 2 with `error: HOME is not set`, never scan the current directory (`Root::Home("")` would resolve to `""`). Test in Task 7.
2. Candidate with an unreadable subdirectory: must be skipped whole, never partially deleted. Test in Task 1 (`measure` returns `Err`) and Task 5.
3. Overlapping rules (a `__pycache__` inside a `~/.cache` child): must appear once, not double-counted or double-deleted. Test in Task 3.
4. Path vanishes between scan and `--apply`: reported as skipped, not as a failure, exit 0. Test in Task 5.
5. Non-UTF-8 file names: table and JSON output must not panic or error (serde_json rejects non-UTF-8 `PathBuf`). Test in Task 4.

---

### Task 1: Scaffold and scanner

**Files:**
- Create: `Cargo.toml`, `.gitignore` (`/target`), `src/lib.rs`, `src/scan.rs`, `src/main.rs` (stub `fn main() {}`)
- Test: `src/scan.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces:
  - `lib.rs`: `pub struct Ctx { pub home: PathBuf, pub tmp: PathBuf, pub proc_root: PathBuf, pub euid: u32, pub now: SystemTime }`. (`from_env` comes in Task 7.)
  - `scan.rs`: `pub enum Matcher { Whole, Children { exclude: &'static [&'static str] }, NamedDirs(&'static str) }`
  - `scan.rs`: `pub struct Entry { pub bytes: u64, pub newest: SystemTime, pub owner: Option<u32>, pub has_special: bool, pub is_symlink: bool }` (`owner` is `Some(uid)` only if every entry in the tree has that uid; `is_symlink` describes the top path only)
  - `scan.rs`: `pub fn find(root: &Path, m: &Matcher) -> Vec<PathBuf>` (missing root returns empty; `Whole` returns `[root]` if it exists; `NamedDirs` does not descend into a match)
  - `scan.rs`: `pub fn measure(path: &Path) -> io::Result<Entry>` (any walk error returns `Err`)

- [ ] **Step 1: `cargo init --lib --name safesweep`, add deps from Tech Stack, add `[[bin]]` via `src/main.rs`, `.gitignore`.**
- [ ] **Step 2: Write failing tests**

```rust
#[test] fn find_missing_root_is_empty()        // find(&tmp.join("nope"), &Matcher::Whole).is_empty()
#[test] fn find_children_respects_exclude()    // root/{a,pip,b} with exclude ["pip"] -> {a,b}
#[test] fn find_named_dirs_no_descend()        // root/x/__pycache__/__pycache__ -> only root/x/__pycache__
#[test] fn find_does_not_follow_symlinks()     // root/link -> outside dir containing __pycache__ -> empty
#[test] fn measure_skips_hardlinked_bytes()    // file a (8 KiB), b = hard_link(a), c (4 KiB, nlink 1): bytes == blocks of c only
#[test] fn measure_newest_is_max_mtime()       // filetime: set one file to now-10d, other now-2d -> newest ≈ now-2d
#[test] fn measure_flags_socket()              // UnixListener::bind(root/s) -> has_special
#[test] fn measure_unreadable_subdir_errs()    // chmod 000 sub; skip test if libc::geteuid()==0; restore perms after
#[test] fn measure_reports_symlink_top()       // measure(link).is_symlink == true, bytes == 0
```

- [ ] **Step 3: Run `cargo test scan`; expect compile failure / FAIL.**
- [ ] **Step 4: Implement `find` and `measure` with walkdir.** `measure` uses `symlink_metadata` + `std::os::unix::fs::{MetadataExt, FileTypeExt}`.
- [ ] **Step 5: Run `cargo test scan`, clippy, fmt; expect PASS.**
- [ ] **Step 6: Commit `feat(scan): find and measure candidates`.**

### Task 2: Guard

**Files:**
- Create: `src/guard.rs`
- Test: `src/guard.rs`

**Interfaces:**
- Consumes: `Ctx`, `Entry` (Task 1).
- Produces:
  - `pub fn in_use(proc_root: &Path) -> io::Result<Vec<PathBuf>>`: `Err` only if `proc_root` itself is unreadable. For each numeric dir: targets of `fd/*`, `cwd`, `exe`, and path column of `maps` (lines whose 6th field starts with `/`). Per-pid errors are ignored.
  - `pub fn check(path: &Path, entry: &Entry, root: &Path, min_age_days: u64, ctx: &Ctx, in_use: Option<&[PathBuf]>) -> Result<(), String>`

Checks in this order, with these exact reasons:

| Condition | Reason |
|---|---|
| `entry.is_symlink` | `symlink` |
| `canonicalize(parent).join(name)` not under `canonicalize(root)` | `outside rule root` |
| `ctx.euid != 0 && entry.owner != Some(ctx.euid)` | `owned by another user` |
| `ctx.now - entry.newest < min_age_days * 86400s` | `modified within {n} days` |
| `entry.has_special` | `contains socket or fifo` |
| `in_use` is `None` | `cannot verify open files` |
| any in-use path `starts_with` the canonical candidate | `in use by a process` |

- [ ] **Step 1: Write failing tests** (build `Ctx` by hand; `proc_root = "/proc"`, `euid = libc::geteuid()`)

```rust
#[test] fn ok_when_old_and_unused()            // aged 10d, min 7 -> Ok(())
#[test] fn skips_symlink()                     // Err("symlink")
#[test] fn skips_outside_root()                // root/sub is symlink -> outside; candidate root/sub/x -> Err("outside rule root")
#[test] fn skips_foreign_owner()               // ctx.euid = real+1 -> Err("owned by another user"); ctx.euid = 0 -> not this error
#[test] fn skips_recent()                      // newest = now-1d, min 7 -> Err("modified within 7 days")
#[test] fn skips_socket()                      // Err("contains socket or fifo")
#[test] fn skips_open_file()                   // keep File::open(cand/f) alive; in_use(/proc) -> Err("in use by a process")
#[test] fn skips_child_cwd()                   // Command::new("sleep").arg("30").current_dir(cand).spawn(); kill after -> Err("in use by a process")
#[test] fn skips_without_proc()                // in_use = None -> Err("cannot verify open files")
#[test] fn in_use_errs_on_missing_proc_root()  // in_use(tmp/"noproc").is_err()
```

- [ ] **Step 2: Run `cargo test guard`; expect FAIL.**
- [ ] **Step 3: Implement `in_use` and `check`.**
- [ ] **Step 4: Run tests, clippy, fmt; expect PASS.**
- [ ] **Step 5: Commit `feat(guard): refuse unsafe candidates`.**

### Task 3: Rules and collect

**Files:**
- Create: `src/rules.rs`
- Modify: `src/lib.rs`
- Test: `src/rules.rs`, `src/lib.rs`

**Interfaces:**
- Consumes: `Matcher`, `find`, `measure` (Task 1); `check`, `in_use` (Task 2).
- Produces:
  - `rules.rs`: `#[derive(Serialize, Clone, Copy, Debug, PartialEq)] #[serde(rename_all = "lowercase")] pub enum Regen { Free, Slow, Network }`
  - `rules.rs`: `pub enum Root { Home(&'static str), Tmp }`
  - `rules.rs`: `pub struct Rule { pub id: &'static str, pub root: Root, pub matcher: Matcher, pub min_age_days: u64, pub regen: Regen }` and `impl Rule { pub fn root_path(&self, ctx: &Ctx) -> PathBuf }` (`Home(p)` is `ctx.home.join(p)`, `Tmp` is `ctx.tmp`)
  - `rules.rs`: `pub static RULES: &[Rule]`
  - `rules.rs`: `pub fn select(ids: &[String]) -> Result<Vec<&'static Rule>, String>` (empty `ids` returns all; unknown returns `Err("unknown category '{id}'; valid: xdg-cache, pip, uv, npm, cargo, pycache, tmp")`)
  - `lib.rs`: `#[derive(Serialize)] pub struct Row { pub category: &'static str, #[serde(serialize_with = ...)] pub path: PathBuf, pub bytes: u64, pub regen: Regen, pub status: String, #[serde(skip)] pub rule: &'static Rule }` (`status` is `"ok"` or `"skipped: {reason}"`; path serialized with `to_string_lossy`)
  - `lib.rs`: `pub fn collect(rules: &[&'static Rule], ctx: &Ctx) -> Vec<Row>`: rayon over rules; `in_use` read once (`.ok()`); `measure` error gives `bytes: 0, status: "skipped: {io error}"`; drop any row whose path is strictly under another row's path; sort by `bytes` descending.

Rule table (exact):

| id | root | matcher | min_age_days | regen |
|---|---|---|---|---|
| `xdg-cache` | `Home(".cache")` | `Children { exclude: &["pip", "uv"] }` | 7 | `Network` |
| `pip` | `Home(".cache/pip")` | `Whole` | 1 | `Network` |
| `uv` | `Home(".cache/uv")` | `Whole` | 1 | `Network` |
| `npm` | `Home(".npm/_cacache")` | `Whole` | 1 | `Network` |
| `cargo` | `Home(".cargo/registry/cache")` | `Whole` | 1 | `Network` |
| `pycache` | `Home("")` | `NamedDirs("__pycache__")` | 0 | `Free` |
| `tmp` | `Tmp` | `Children { exclude: &[] }` | 7 | `Free` |

Add above `pycache`: `// shortcut: walks all of $HOME single-threaded per rule; switch to jwalk if scans of large homes are slow.`

- [ ] **Step 1: Write failing tests**

```rust
#[test] fn select_all_when_empty()           // select(&[]).unwrap().len() == 7
#[test] fn select_unknown_errs()             // select(&["nope".into()]) == Err("unknown category 'nope'; valid: xdg-cache, pip, uv, npm, cargo, pycache, tmp")
#[test] fn collect_fixture_home()            // home/.cache/{old(10d), pip/x(10d)}, home/p/__pycache__/a.pyc
                                             // -> rows: xdg-cache old ok, pip ok, pycache ok; no xdg-cache row for pip
#[test] fn collect_dedupes_nested()          // home/.cache/old/__pycache__ (10d) -> exactly one row, category "xdg-cache"
```

- [ ] **Step 2: Run `cargo test`; expect FAIL.**
- [ ] **Step 3: Implement `rules.rs` and `collect`.**
- [ ] **Step 4: Run tests, clippy, fmt; expect PASS.**
- [ ] **Step 5: Commit `feat(rules): add rule table and collect rows`.**

### Task 4: Render

**Files:**
- Create: `src/render.rs`
- Test: `src/render.rs`

**Interfaces:**
- Consumes: `Row` (Task 3).
- Produces: `pub fn human(bytes: u64) -> String`, `pub fn table(rows: &[Row]) -> String`, `pub fn json(rows: &[Row]) -> String` (`serde_json::to_string_pretty`).

`human`: binary units `B, KiB, MiB, GiB, TiB`; bytes under 1024 shown as integer (`"0 B"`, `"512 B"`), else one decimal (`1536` gives `"1.5 KiB"`). `table`: header `CATEGORY  SIZE  REGEN  STATUS  PATH`, columns padded to widest value, path via `display()`, final line `total reclaimable: {human(sum of bytes where status == "ok")}`.

- [ ] **Step 1: Write failing tests**

```rust
#[test] fn human_units()       // 0->"0 B", 512->"512 B", 1536->"1.5 KiB", 5*1024^3->"5.0 GiB"
#[test] fn table_total_ok_only()  // one ok row 2048, one skipped 4096 -> ends with "total reclaimable: 2.0 KiB"
#[test] fn json_non_utf8_path()   // PathBuf::from(OsStr::from_bytes(b"bad\xff")) -> json() does not panic, contains "bad"
```

- [ ] **Step 2: Run; expect FAIL.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run tests, clippy, fmt; expect PASS.**
- [ ] **Step 5: Commit `feat(render): table and json output`.**

### Task 5: Clean

**Files:**
- Create: `src/clean.rs`
- Test: `src/clean.rs`

**Interfaces:**
- Consumes: `Row`, `Ctx` (Tasks 1, 3); `measure` (Task 1); `check`, `in_use` (Task 2); `human` (Task 4).
- Produces: `pub struct Outcome { pub freed: u64, pub removed: usize, pub failed: usize, pub lines: Vec<String> }` and `pub fn clean(rows: &[Row], ctx: &Ctx, apply: bool, manifest_dir: &Path) -> io::Result<Outcome>`.

Behavior:
- Only rows with `status == "ok"` are considered.
- Dry run: line `would remove {human} {path}` per row; `freed` = sum; no filesystem changes; no manifest.
- Apply: `create_dir_all(manifest_dir)` and create `{manifest_dir}/{unix_secs}.log` first; on error return `Err` before deleting anything. Take one `in_use` snapshot (`.ok()`) (`// shortcut: one /proc snapshot per run, not per file; take a fresh one per candidate if races show up.`). Per row: `measure` + `check` again; failure gives line `skipped {path}: {reason}` (not counted as failed). Otherwise `remove_dir_all` (dir) or `remove_file`. Success: manifest line `{bytes}\t{path}`, line `removed {human} {path}`, add to `freed`/`removed`. Error: manifest line `ERROR\t{path}\t{err}`, line `failed {path}: {err}`, `failed += 1`.

- [ ] **Step 1: Write failing tests**

```rust
#[test] fn dry_run_deletes_nothing()       // path still exists; freed == bytes; manifest_dir has no files
#[test] fn apply_removes_ok_rows_only()    // ok row removed, skipped row kept; manifest has "{bytes}\t{path}" line
#[test] fn apply_skips_vanished_path()     // delete the path before clean -> failed == 0, line starts "skipped"
#[test] fn apply_rechecks_open_files()     // row ok, then hold File::open inside it -> not removed, line starts "skipped"
#[test] fn apply_aborts_without_manifest()  // manifest_dir = a regular file -> Err, candidate still exists
```

- [ ] **Step 2: Run; expect FAIL.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run tests, clippy, fmt; expect PASS.**
- [ ] **Step 5: Commit `feat(clean): guarded removal with manifest`.**

### Task 6: WSL hint

**Files:**
- Create: `src/wsl.rs`
- Test: `src/wsl.rs`

**Interfaces:**
- Produces: `pub fn is_wsl(proc_version: &str) -> bool` (case-insensitive contains `microsoft`), `pub fn hint(distro: &str) -> String`.

`hint` returns exactly (with `{distro}` substituted):

```text
WSL: freed space stays inside ext4.vhdx until you compact it from Windows.
In PowerShell (as admin):
  1. Back up:   wsl --export {distro} {distro}-backup.tar
  2. Stop WSL:  wsl --shutdown
  3. Find the disk:
     (Get-ChildItem HKCU:\Software\Microsoft\Windows\CurrentVersion\Lxss | Get-ItemProperty | Where-Object DistributionName -eq '{distro}').BasePath
     The file is ext4.vhdx in that folder.
  4. Compact:   Optimize-VHD -Path <folder>\ext4.vhdx -Mode Full
     (no Hyper-V? diskpart: select vdisk file="<folder>\ext4.vhdx", attach vdisk readonly, compact vdisk, detach vdisk)
```

- [ ] **Step 1: Write failing tests**: `is_wsl` true for the real string `Linux version 6.18.54.1-microsoft-standard-WSL2`, false for `Linux version 6.8.0-45-generic`; `hint("Ubuntu")` contains `wsl --export Ubuntu Ubuntu-backup.tar` and `DistributionName -eq 'Ubuntu'` and does not contain `set-sparse`.
- [ ] **Step 2: Run; expect FAIL.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run tests, clippy, fmt; expect PASS.**
- [ ] **Step 5: Commit `feat(wsl): add vhdx compaction hint`.**

### Task 7: CLI, integration test, release docs

**Files:**
- Modify: `src/main.rs`, `src/lib.rs`
- Create: `tests/cli.rs`, `README.md`, `LICENSE-MIT`, `LICENSE-APACHE`
- Modify: `Cargo.toml` (`license = "MIT OR Apache-2.0"`, `description`, `repository` left out until a remote exists)

**Interfaces:**
- Consumes: everything above.
- Produces: `impl Ctx { pub fn from_env() -> Result<Ctx, String> }`: `HOME` unset or empty gives `Err("HOME is not set")`; `tmp = std::env::temp_dir()`; `proc_root = "/proc"`; `euid = unsafe { libc::geteuid() }`; `now = SystemTime::now()`.

CLI (clap derive):

```text
safesweep [scan] [--category <ID>]... [--json]
safesweep clean [--category <ID>]... [--apply]
```

`--category` is a global repeatable arg. No subcommand means `scan`. Errors print `error: {msg}` to stderr. `clean` prints `Outcome.lines`, then `freed {human(freed)}` (`would free` on a dry run). After `--apply` with `freed > 0` and `is_wsl(/proc/version)`: print `hint(WSL_DISTRO_NAME or "<distro>")`. Exit 1 if `failed > 0` or `clean` returned `Err`.

- [ ] **Step 1: Write failing integration tests** (`assert_cmd`; env `HOME` = fixture home, `TMPDIR` = fixture tmp; helper `age(path, days)` sets mtime on every entry bottom-up with filetime)

```rust
#[test] fn scan_json_statuses()     // fixture: .cache/old (10d), .cache/fresh (now), .cache/held/f (10d, held open by test),
                                    // proj/__pycache__/a.pyc, tmp/stale (10d)
                                    // -> old ok, fresh "skipped: modified within 7 days", held "skipped: in use by a process",
                                    //    pycache ok, tmp stale ok
#[test] fn clean_dry_run_then_apply()  // dry run: all still exist, exit 0, stdout contains "would free"
                                       // apply: old, __pycache__, stale gone; fresh, held remain;
                                       // one file in HOME/.local/state/safesweep/ listing the 3 removed paths; exit 0
#[test] fn unknown_category_exits_2()  // `--category nope` -> exit 2, stderr contains "unknown category 'nope'"
#[test] fn missing_home_exits_2()      // env_remove("HOME") -> exit 2, stderr "error: HOME is not set"
```

- [ ] **Step 2: Run `cargo test --test cli`; expect FAIL.**
- [ ] **Step 3: Implement `from_env` and `main.rs`.**
- [ ] **Step 4: Run full `cargo test`, clippy, fmt; expect PASS.**
- [ ] **Step 5: Write `README.md`**: one-line pitch ("Clean caches and temp files that are provably safe to delete. Nothing open, nothing recent, nothing shared."), install (`cargo install safesweep`), usage of the 3 commands, the safety checks as a list, the rule table, the WSL section, license line. Add both license files with standard text, copyright `2026 Kaitlyn Bassford`.
- [ ] **Step 6: Manual check on this WSL distro: `cargo run -- scan`, then `cargo run -- clean`. Do not run `--apply` outside the test fixtures without asking the user.**
- [ ] **Step 7: Commit `feat(cli): add scan and clean commands`, then `docs: add readme and licenses`.**
