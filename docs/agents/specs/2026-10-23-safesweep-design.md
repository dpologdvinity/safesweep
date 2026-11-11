# safesweep: design

Name chosen 2026-10-23; free on crates.io and Homebrew at that date.

## Goal

A Rust CLI for developers on WSL (and plain Linux) that finds caches and temp files that are provably safe to delete, shows what they cost to regenerate, and removes them on request. Later stages (see Roadmap) add project-artifact cleaning, an explorer TUI, Docker pruning, and CPU attribution. This spec covers stage 1 only.

Success: the user trusts it enough to run `safesweep clean --apply` without reviewing each path, and it is distinct enough from `dust`, `dua`, `kondo`, and BleachBit to attract stars.

## Distinguishing features

1. **Rule-based, not heuristic.** Only known locations are ever candidates. Nothing is guessed from size or age alone.
2. **Open-file safety.** Anything held open by a running process, or used as a process working directory, is never deleted.
3. **WSL awareness.** On WSL, after a clean, explain that freed space stays inside `ext4.vhdx` until the disk is compacted from Windows, and print the compaction steps. Never run them automatically. No existing tool joins in-WSL cleaning with Windows-side reclaim (wslcompact works from Windows only).

Positioning: lead with "safe-by-proof cleaning plus WSL reclaim". Explorer TUIs and `node_modules` cleaners are saturated, so they come later as stages, not as the pitch.

## Roadmap

The long-term aim is one tool covering what `kondo`, `npkill`, BleachBit, `dua`/`dust`/`ncdu`, and `docker system prune` do separately. Each stage ships as its own release and reuses the stage 1 scanner and rule table. Only stage 1 is specified and planned here; later stages get their own spec.

1. **Core (this spec):** parallel scanner, rule table, open-file guard, `scan` and `clean`, WSL compaction hint.
2. **Project artifacts:** per-project build dirs (`node_modules`, `target`, `.venv`, `dist`), staleness from last git activity.
3. **Explorer TUI:** size-sorted browsing, treemap view, delete from the UI, safe badges from the rules.
4. **Docker and package managers:** images, layers, build cache.
5. **`why`:** CPU attribution by process tree and tool.

## Out of scope for v1 (stage 1)

- Everything in stages 2 to 5, anything on the Windows side, macOS support, Windows-native support, configuration files.

## CLI

- `safesweep` or `safesweep scan`: print a table of candidates with columns category, path, size, regeneration cost (`free`, `slow`, `network`), and status (`ok` or `skipped: <reason>`). Deletes nothing.
- `safesweep clean`: dry run by default. Prints what would be removed and the total.
- `safesweep clean --apply`: removes candidates whose status is `ok`.
- `--category <id>` (repeatable) limits scan and clean to the named rules.
- `--json` on `scan` prints machine-readable output.
- Exit code 0 on success, 1 on a partial failure during `--apply`, 2 on bad usage.

## Components

- **Rule table** (`rules.rs`): a static list. Each rule has `id`, `root` (path with `~` expansion), `matcher` (whole directory, each child of the root, or every directory with a given name), minimum age in days, and `regen` cost. Initial rules: `~/.cache` children other than `pip` and `uv` (7 days), `~/.cache/pip` and `~/.cache/uv` (1 day), npm `~/.npm/_cacache` (1 day), cargo `~/.cargo/registry/cache` (1 day), `__pycache__` directories under `~` (0 days, opt-in: runs only with `--category pycache`, because walking all of `~` took 9+ minutes on a real dev machine), and temp-dir children (7 days). No stage 1 rule needs root; other users' files are skipped by the owner check. Stale `node_modules` moves to stage 2, where staleness comes from git activity rather than mtime.
- **Scanner** (`scan.rs`): for each rule, walks only under its root in parallel, sums sizes, and produces candidates. Later stages reuse it, so it takes a root and a matcher and knows nothing about specific rules. Does not follow symlinks. Does not cross filesystem boundaries. Size counts only files with a single hard link, measured in allocated blocks: uv hardlinks its cache into venvs, so deleting a hardlinked file frees nothing and must not be reported as freed.
- **Guard** (`guard.rs`): takes a candidate and returns `ok` or a skip reason. Checks, in order: not a symlink; path still under the rule root after canonicalization; not owned by another user (unless running as root); nothing inside modified within the rule's age threshold; contains no sockets or FIFOs (open sockets show in `/proc` as `socket:[inode]`, not as paths, so a live tmux or X11 socket would otherwise look unused); nothing inside is open by any process (`/proc/*/fd`), is any process's cwd or exe, or is memory-mapped (`/proc/*/maps`). Processes whose `/proc` entries are unreadable (other users' processes when not root) are ignored.
- **Cleaner** (`clean.rs`): re-runs the guard immediately before each removal, then removes. Appends a line per removal to a manifest at `~/.local/state/safesweep/<timestamp>.log`.
- **WSL** (`wsl.rs`): detects WSL from `/proc/version`. Reads `WSL_DISTRO_NAME`. Prints, in order: back up with `wsl --export <distro> <file>`; run `wsl --shutdown`; compact with `Optimize-VHD` (Hyper-V) or `diskpart` `compact vdisk`. It does not recommend `--set-sparse`: WSL 2.5.6 disabled it behind `--allow-unsafe` over data-corruption reports, it was only re-enabled as experimental in the 3.0.2 pre-release, and it blocks `Optimize-VHD`.
- **Output** (`main.rs`, `render.rs`): argument parsing (clap) and table or JSON rendering.

## Data flow

`rules` → `scan` (candidates with sizes) → `guard` (status per candidate) → `render` for `scan`; or → `clean` (guard again, remove, write manifest) → `wsl` hint.

## Error handling

- Permission denied or a vanished file during scan: record as `skipped: <reason>` and continue.
- Failure during removal: log it, continue with the rest, exit code 1 at the end.
- The guard runs twice (scan and just before removal) so a process that opens a file between the two cannot lose it.
- If `/proc` cannot be read, the open-file check cannot be trusted. The guard then marks every candidate `skipped: cannot verify open files`.

## Testing

- Unit tests for the guard using temp directories: open file held by the test process, cwd held by a child process, symlink escaping the root, recent mtime, wrong owner.
- Scanner tests against a fixture tree for size totals and age matching.
- One integration test: build a fixture home, run `clean` (dry run, then `--apply`), assert exactly the expected paths were removed and the manifest lists them.
- Manual check on a real WSL distro before release.

## Open questions

- Verify how BleachBit detects in-use files before claiming the open-file guard is unique.
- License: dual `MIT OR Apache-2.0` (Rust ecosystem convention) unless the user prefers otherwise.
