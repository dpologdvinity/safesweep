# Architecture

This document explains how `safesweep` is built and, more importantly, *why*. It is for people who
want to change the code or to judge whether its safety claims hold. For how to *use* the tool, read
the [README](README.md). For the API of each module, run `cargo doc --open`: every item is documented.

- [What it is and what it is not](#what-it-is-and-what-it-is-not)
- [Technology](#technology)
- [Repository layout](#repository-layout)
- [The big picture](#the-big-picture)
- [Components and their boundaries](#components-and-their-boundaries)
- [Data flow](#data-flow)
- [The safety model](#the-safety-model)
- [Rules: the data that drives the tool](#rules-the-data-that-drives-the-tool)
- [Output and untrusted text](#output-and-untrusted-text)
- [The interactive explorer (`tui`)](#the-interactive-explorer-tui)
- [CPU report (`why`)](#cpu-report-why)
- [Errors and exit codes](#errors-and-exit-codes)
- [Concurrency and performance](#concurrency-and-performance)
- [Testing](#testing)
- [Packaging and releases](#packaging-and-releases)
- [Design decisions](#design-decisions)
- [Known limits](#known-limits)

## What it is and what it is not

`safesweep` deletes cache and temporary files, and only when it can show that deleting them is safe.
The one thing that must never happen is a **false "safe"**: reporting something deletable that a
program is using, that someone just created, or that belongs to someone else. Everything in the
design serves that, even where it costs speed or reach. When in doubt the tool skips and says why.

It is:

- a single-user command-line tool for **Linux and WSL**, run by the person whose files they are;
- **rule-based**: it only ever considers locations that a rule names (see [Rules](#rules-the-data-that-drives-the-tool)).

It is not:

- a general disk-usage analyser: it will not find "large old files" or touch a path no rule covers;
- a background service or a security boundary against a hostile user *with the same account*;
- portable to macOS or Windows: it reads `/proc` to see what programs are using.

## Technology

| Part | Choice | Why |
|---|---|---|
| Language | Rust, edition 2024, minimum Rust 1.88 (`rust-version` in `Cargo.toml`, tested in CI) | memory safety, one static binary, good filesystem APIs |
| Shape | one crate: a library (`src/lib.rs`) and a thin binary (`src/main.rs`) | logic is testable without a process; the binary only parses arguments and prints |
| CLI | `clap` (derive) + `clap_complete` + `clap_mangen` | help, completions and the man page come from one definition and cannot drift |
| Terminal UI | `ratatui` (with its crossterm backend) | the `tui` screen; kept out of the safety-critical code |
| Parallelism | `rayon` | scans many rules and candidates at once |
| Directory walks | `walkdir` | never follows symlinks, can stay on one filesystem |
| Output | `serde` + `serde_json` | `scan --json` |
| System | `libc` | `geteuid`, `makedev` |
| Distribution | static `musl` binaries, `.deb`, `.rpm`, crates.io, PyPI (maturin), npm, Homebrew, Nix | see [Packaging and releases](#packaging-and-releases) |

## Repository layout

```text
src/
  main.rs     command line: argument parsing, one function per subcommand, exit codes
  lib.rs      crate docs; Ctx (the environment), Row (one candidate), collect() (find + judge all)
  rules.rs    the rule table (data) and rule selection
  scan.rs     FIND candidates and MEASURE them (observes only; never changes anything)
  guard.rs    DECIDE: the only place that says whether a candidate may be deleted
  clean.rs    ACT: the only code that deletes; re-checks every candidate first; writes the log
  render.rs   output: table, JSON, sizes, and escaping of untrusted text
  tui.rs      the interactive explorer (state machine + drawing)
  why.rs      CPU report from /proc (independent of the cleaning code)
  wsl.rs      WSL detection and the disk-compaction instructions
tests/cli.rs  end-to-end tests: the real binary against a fake home
npm/          npm launcher (Node) + build script + tests
apt/          builds the signed apt repository
packaging/    generates completions and the man page for the packages
demo/         rebuilds the README animation
.github/      CI, release, PyPI, npm and tag-check workflows
Cargo.toml    metadata, dependencies, and the .deb/.rpm/binstall settings
pyproject.toml, flake.nix   PyPI and Nix packaging
```

## The big picture

```text
        ┌───────────────────────── safesweep (library) ─────────────────────────┐
        │                                                                       │
 rules ─┼─► scan::find ─► scan::measure ─► guard::check ─► Row ─► render        │
 (what) │   candidates     what is in it    safe? why not?   │     (print)      │
        │                                                     ├───► tui          │
        │                                                     │     (choose)     │
        │                                                     └───► clean::clean │
        │                          re-measure + re-check, then delete + log      │
        └───────────────────────────────────────────────────────────────────────┘
          why (CPU report) shares nothing with the above except render's escaping
```

The central idea is a three-way split of responsibilities:

| Verb | Module | May | May not |
|---|---|---|---|
| **Observe** | `scan` | read the filesystem | decide anything, change anything |
| **Decide** | `guard` | say yes or no, with a reason | change anything |
| **Act** | `clean` | delete | decide: it asks `guard` again, right before each removal |

Because only `clean` deletes and only `guard` decides, safety review means reading two functions
(`guard::check` and `clean::clean`) and not the whole program. The interactive `tui` has no way to
delete except by calling `clean::clean`, so the interface cannot do anything the command line would
refuse.

## Components and their boundaries

**`rules` (what to clean).** A `Rule` is plain data: an id, a root directory (relative to `$HOME`
or the temp directory), a `Matcher`, a minimum age in days, what it costs to regenerate, and whether
it is opt-in. There is no behavior in it. Adding a location means adding one entry to `RULES`.

**`scan` (find and measure).** `find(root, matcher)` returns candidate paths. `measure(path)`
walks one candidate and returns an `Entry`: bytes that deleting would free, newest change time,
owner, whether it holds a socket or pipe, whether it is a symlink, and the `(device, inode)` of
everything inside. `measure` *fails* if the tree cannot be fully seen or would not delete cleanly:
an unreadable entry, a mount point, a nested `.git`, a read-only directory. An error here is
reported to the user as the skip reason.

**`guard` (decide).** `guard::in_use` reads `/proc` once and lists what running processes have
open, map, execute or use as working directory. `guard::check` combines that with the `Entry` and
returns `Ok(())` or a reason. The order and rationale of the checks are in
[The safety model](#the-safety-model).

**`clean` (act).** For each row that was `ok`, `clean`: re-measures it, re-runs `guard::check` with a
fresh enough process list, deletes it (`remove_dir_all` for a directory, `remove_file` otherwise),
and appends a line to the manifest. It returns an `Outcome` for the caller to print.

**`lib` (glue).** `Ctx` gathers everything environmental (home, temp dir, `/proc` location, user id,
clock) and validates it once, so the rest of the code never reads the environment, and tests can
supply a fake home, a fake `/proc` and a moved clock. `collect` runs find, measure and check for a
list of rules and returns `Row`s, largest first, with overlaps removed.

**`render`, `tui`, `why`, `wsl`, `main`** are at the edge: they present results and take input, and
contain no safety decisions.

Dependencies point one way: `main`/`tui` → `clean`/`collect` → `guard`/`scan`/`rules`. Nothing in
`scan`, `guard` or `rules` knows about output or the interface.

## Data flow

### `safesweep scan` / `safesweep` (no arguments)

1. `main` builds a `Ctx` from the environment (`Ctx::from_env`, which validates `HOME`/`TMPDIR` and
   refuses unsafe root use) and selects rules (`rules::select`).
2. `collect` reads `/proc` and the mount table once, then for each rule (in parallel):
   `scan::find` → for each candidate (in parallel) `scan::measure_with` → `guard::check`.
   Every candidate becomes a `Row` whose `status` is `"ok"` or `"skipped: <reason>"`.
3. Rows that lie inside another row (found by a different rule, or through a symlinked directory)
   are dropped, keeping the outermost, so nothing is counted or deleted twice.
4. `render::table` or `render::json` prints them. Nothing was changed.

### `safesweep clean [--apply]`

Same as above up to the rows. Then `clean::clean`:

- **Dry run** (no `--apply`): list what would be removed. No manifest is written.
- **With `--apply`:** create the manifest *first* (if that fails, delete nothing), then for each `ok`
  row: re-measure, re-check (`guard::check`), delete, log. A row that no longer passes is reported as
  `skipped` and left alone. If the log stops being writable after a deletion, report it and stop.
- On WSL, after a real cleanup, print the disk-compaction instructions.

### `safesweep tui`

`tui::run` scans in the background (cancellable), builds an `App`, and loops: draw, read a key,
apply it. Deleting calls `clean::clean` on the selected rows, then rescans. See
[The interactive explorer](#the-interactive-explorer-tui).

### `safesweep why`

Takes two samples of `/proc` a moment apart and prints the difference. Independent of everything
above. See [CPU report](#cpu-report-why).

## The safety model

### Threats considered

| Threat | Example | Defence |
|---|---|---|
| A program is using the files | a browser with its cache open; a build with its `target/` | [in-use check](#the-checks-in-order), by path and by inode |
| The files are new | an archive unpacked a minute ago, with old timestamps | age uses the newer of modification time and inode change time |
| The files are not the user's | another user's directory in `/tmp` | owner must match (ordinary users) |
| The path is not what it seems | a symlink, `~/.cache` pointing at `/var/lib`, `..` | symlinks never followed; resolved paths must stay in the rule's root; root refuses symlinked roots |
| Deleting would escape or break | a mount point or bind mount inside; a read-only directory (deletion stops halfway) | `measure` refuses |
| Hostile file names | a name with a newline or escape codes faking output | all text is escaped before printing ([render](#output-and-untrusted-text)) |
| Misuse as root | `sudo -E safesweep` keeps the user's `HOME` | refuses a `HOME` not owned by root; refuses the temp dir |
| Bad environment | empty or relative `HOME`, `TMPDIR=/` | refused at startup (`Ctx::from_env`) |
| Losing the record | disk full while logging | log opened before deleting; stops if it breaks |

### The checks, in order

`guard::check` runs these and the first failure wins. Each returns the reason shown to the user.

1. **Not a symlink.** A link is described, never followed.
2. **Inside the rule's root**, after resolving symlinks in the *parent* (never the candidate). As
   root, the root itself must not be reached through a symlink.
3. **Not the temp directory or an ancestor of it.** Deleting `$TMPDIR` breaks every tool.
4. **As root, not in the temp directory.** For root, `/proc` shows paths of containers and services
   with a private `/tmp` in *their* view, so their live files cannot be recognised.
5. **As an ordinary user, every file belongs to that user.**
6. **Nothing changed within the rule's minimum age.** "Changed" is the newer of mtime and ctime: tools
   like `tar -p` back-date mtime, but ctime cannot be set by any program.
7. **No sockets or named pipes inside.** A live socket (tmux, X11) looks like a closed file because
   a socket is not listed under a path in `/proc`.
8. **The process list is available.** If `/proc` could not be read, nothing is "safe".
9. **No process uses the tree, or its scope, by path or by inode.** Matching by inode is what makes
   this work for processes in containers or other mount namespaces, which see different paths. For
   build directories the *scope* is the whole project, because a running server has its working
   directory in the project, not inside `node_modules`.

### What is guaranteed and what is not

- **Guaranteed:** nothing is deleted unless `guard::check` passed for it in the same run, a moment
  earlier; the run never follows a symlink out of a root; every deletion is logged (or the run stops).
- **Not guaranteed:** there is an unavoidable gap between "checked" and "removed". A program could open
  a file in that gap. The gap is kept small: the check runs just before the removal, and the process
  list is re-read at most once per second (`guard::SnapshotCache`). This is a deliberate trade: re-reading
  `/proc` for every candidate made a 300-entry cleanup take 35 seconds instead of 3. On Linux a program that
  already holds a file open keeps working after the file is unlinked, which limits the damage.
- **Not protected against:** another user's processes when not running as root (they cannot be read),
  and a hostile program under the *same* account (it could do the same damage itself).

### Why `measure` fails instead of skipping

When a candidate cannot be fully measured, the safe answer is "unknown", and unknown means "do not
delete". So `measure` returns an error, `collect` shows it as the skip reason, and `clean` never sees
it as `ok`. The same goes for every uncertainty: no `/proc`, no mount table, unresolvable path.

## Rules: the data that drives the tool

| Id | Root | Matcher | Min age | Opt-in |
|---|---|---|---|---|
| `xdg-cache` | `~/.cache` | each child except `pip`, `uv` | 7 days | no |
| `pip` | `~/.cache/pip` | the directory | 1 day | no |
| `uv` | `~/.cache/uv` | the directory | 1 day | no |
| `npm` | `~/.npm/_cacache` | the directory | 1 day | no |
| `cargo` | `~/.cargo/registry/cache` | the directory | 1 day | no |
| `tmp` | `$TMPDIR` or `/tmp` | each child | 7 days | no |
| `pycache` | `~` | every `__pycache__` (not entering hidden dirs) | 0 | **yes** |
| `artifacts` | `~` | `node_modules`, `target`, `.venv` of idle git projects | 30 days | **yes** |

Rules walking the whole home directory are opt-in: they are slow and `artifacts` deletes things that
take minutes to rebuild. Ages are short where refilling is cheap (package downloads) and longer where
deletion hurts more.

**The `artifacts` matcher** is the most careful. A build directory counts only if (a) a project marker
file sits next to it (`package.json`, `Cargo.toml`, `pyproject.toml`, ...), (b) a file only the tool
creates sits inside it (`pyvenv.cfg`, `CACHEDIR.TAG`, `.package-lock.json`, ...) so a hand-made
folder named `target` or `.venv/src/<package>` checkouts are never mistaken for build output, and (c)
the project's git activity is older than 30 days. Git activity is the newest modification time of the
files git touches: the main checkout, every branch's reflog, and every linked worktree. A project
without a readable `.git` has *unknown* activity, which counts as *not idle*.

**To add a rule:** add a `Rule` to `RULES` in `src/rules.rs`; pick or add a `Matcher` in
`src/scan.rs`; add the id to the expected lists in the `rules` tests and to the README table. No other
code changes, and every safety check applies automatically.

## Output and untrusted text

File names, I/O error messages, process names and command lines are all chosen by someone else. A
name containing a newline would draw a fake table row; one containing an escape character could drive
the terminal; invalid UTF-8 would break JSON. So everything that came from the filesystem or from
`/proc` passes through `render::escape_path` / `render::escape_text` before it is printed, logged or
put in JSON. Escaped output never contains a raw control character, newlines become `\n`, a literal
backslash becomes `\\` (so the two never look alike), and invalid bytes become `\xNN`. The manifest and
`scan --json` use the same escaping. This is why output and log lines are always one line per path.

## The interactive explorer (`tui`)

`tui.rs` is a small state machine, separate from the screen:

- `App` holds the rows, the cursor, the selection (only `ok` rows can be selected), the "confirm?"
  flag and a status message. `App::handle_key` maps a key to a new state and returns whether to quit.
- `draw` paints an `App`; it has no side effects. Both are tested with an in-memory terminal.
- `run` owns the real terminal, scans in a background thread (so `q` can cancel a scan; raw mode
  swallows Ctrl-C), and rescans after each delete.
- `translate` accepts only plain key presses, so Ctrl-D or a held key cannot open or confirm the
  delete prompt; Ctrl-C quits.
- Rows owned by other users are hidden and counted: they can never be deleted and would only let
  another user fill the list with junk.

Deleting is `clean::clean` on the selected rows, the same function as `clean --apply`.

## CPU report (`why`)

`why::sample` reads `/proc/stat` (machine-wide tick counters) and `/proc/<pid>/stat` for every
process. `why::diff` compares two samples: a process's percentage is its tick difference divided by
the ticks one core produced in the interval (so a busy process on 12 cores can show 1200%). It ignores a
process whose start time changed (a reused process id), excludes guest ticks (already counted as user
time), and reports nothing when no time passed (no division by zero). `main` prints the totals per
program and the busiest processes; command lines are escaped. On WSL it notes that Windows-side load
(`vmmem`) is not visible from inside.

## Errors and exit codes

Internal functions return `Result` with a short message; the message *is* the skip reason a user sees.
`main` maps outcomes to exit codes:

| Code | Meaning |
|---|---|
| 0 | success |
| 1 | the command ran but something failed: a removal failed or could not be logged, `/proc` or the terminal was unusable |
| 2 | bad usage: unknown `--category`, unsafe `HOME` or `TMPDIR`, a flag that does not apply to the subcommand |

Input from the filesystem and from `/proc` is never trusted to be well formed: unreadable, vanishing or
malformed entries become skip reasons (or are left out of the CPU report) instead of errors, and `why | head`
(a closed pipe) is not an error.

## Concurrency and performance

`collect` scans rules in parallel and, inside a rule, judges candidates in parallel (`rayon`). This
matters because a rule such as `pycache` can find over a thousand candidates, each needing several
system calls. The mount table and the process list are read once per scan and shared. `clean` deletes
sequentially, on purpose: it is I/O bound, simple to reason about, and a log line per removal stays
in order.

## Testing

| Layer | Where | What it proves |
|---|---|---|
| Unit | `#[cfg(test)]` in each module | each rule of the guard, scan, rendering, parsing, the TUI state machine |
| Integration | `tests/cli.rs` | the real binary: exit codes, output, the manifest, the opt-in rules, the refusals |
| Launcher | `npm/test.js` | the npm launcher passes arguments, exit codes and signals through |
| Static | CI | `rustfmt`, `clippy -D warnings` (including `missing_docs`), `rustdoc -D warnings`, `shellcheck`, `actionlint`, `cargo audit` |
| Coverage | CI | line coverage must stay at or above 90% (about 95% when set) |

Two things shape the fixtures:

- **Time.** A file's inode change time cannot be back-dated, so tests that need an "old" tree move the
  *clock* forward instead (`Ctx::now`). The end-to-end tests run the real binary on the real clock, so
  they use the zero-day `pycache` rule or check that recent files are *refused*.
- **Namespaces.** The mount-point and container tests use `unshare -rm` (unprivileged mount
  namespaces). On CI they must run: being unable to use namespaces is a failure there, not a silent
  skip, so a regression test can never quietly stop running.

The workflow is test first: a safety bug gets a test that fails before the fix.

## Packaging and releases

One binary, many channels. The release workflow builds static `musl` binaries (x86_64 and aarch64),
`.deb` and `.rpm` packages, completions and a man page generated from the program itself. Each
channel then reuses *those* binaries:

| Channel | How it is built | Workflow |
|---|---|---|
| GitHub release | tarballs + `.sha256`, `.deb`, `.rpm` | `release.yml` |
| crates.io | `cargo publish` (trusted publishing, else token) | `release.yml` |
| apt repository | `apt/build.sh`, signed, served from GitHub Pages | `release.yml` |
| PyPI | `maturin` wheels (glibc and musl) and sdist | `pypi.yml` |
| npm / pnpm / yarn / bun | `npm/build.sh` repackages the release binaries | `npm.yml` |
| Homebrew | formula in `dpologdvinity/homebrew-tap`, regenerated from the release; `release.yml` triggers it | tap repo |
| Nix | `flake.nix`, built in CI | `ci.yml` |
| cargo-binstall | `[package.metadata.binstall]` points at the release tarballs | none |

Safeguards:

- **Tag gate.** Every publishing workflow first calls `verify-tag.yml`: the tag must be `vX.Y.Z[-pre]`
  and equal the version in `Cargo.toml`. (A release once got the tag `v`; this gate exists so that
  cannot publish anything again.)
- **Never replace.** Release assets are not overwritten (rebuilt binaries differ byte for byte, and Homebrew
  and apt pin checksums). A missing `.sha256` is derived from the tarball already published.
- **Idempotent.** Re-running a workflow skips what is already published.
- **Least privilege.** Actions are pinned to commits; each job declares its own `permissions`; checkouts
  keep no credentials.
- **Prereleases** go to npm under `next` and skip apt and Homebrew.

How to cut a release: [RELEASING.md](RELEASING.md).

## Design decisions

1. **Rules, not heuristics.** A cleaner that guesses ("big and old, probably junk") will eventually delete
   something that mattered. Naming exactly what may be cleaned makes the tool's claim checkable.
2. **Observe, decide, act as separate modules.** It keeps the dangerous code small enough to review.
3. **Re-check at deletion time.** A scan can be minutes old; the world changes. The earlier verdict is a
   report, not a permission.
4. **Skip on doubt.** Every uncertainty becomes a skip with a reason. The cost is that something deletable
   is sometimes left; the benefit is that nothing undeletable is ever taken.
5. **Match by inode as well as by path.** Paths differ across mount namespaces; inodes do not.
6. **ctime as well as mtime.** mtime can be set; ctime cannot.
7. **`measure` refuses mounts and nested `.git`.** Removing a tree that contains a mount would delete the
   mounted data; a nested checkout may hold uncommitted work.
8. **Opt-in for slow or heavy rules.** Defaults stay fast and low-risk.
9. **A 1-second process snapshot.** Chosen by measurement (35 s → 3 s for 300 candidates) over a fresh
   read per candidate; the added window is documented and small.
10. **Static binaries and one launcher per ecosystem.** Users get the same program from any channel, with
    no runtime to install.
11. **Generated completions and man page.** They come from the same definition as `--help`.
12. **Linux only.** The in-use check depends on `/proc`. A port would need another source of "what is open".

## Known limits

- Other users' processes are invisible unless run as root; their files are already refused by the owner check.
- A tiny window between check and removal remains (see [What is guaranteed](#what-is-guaranteed-and-what-is-not)).
- `pycache` and `artifacts` walk the home directory and can take seconds to minutes on large trees.
- The apt repository serves only the latest stable release.
- No macOS or Windows support.
