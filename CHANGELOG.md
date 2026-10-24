# Changelog

Versions follow [semver](https://semver.org). Before 1.0, a minor version can change behavior.

## 0.6.1 - 2026-10-23

Release pipeline hardening from a review of the packaging; no change to what `scan` and `clean` do.

### Fixed
- npm launcher forwards SIGTERM/SIGINT/SIGHUP to the native process (before, `timeout 60 safesweep clean --apply` kept deleting after the timeout).
- Nothing publishes unless the tag is `vX.Y.Z[-pre]` and matches `Cargo.toml` (before, only the crates.io job checked).
- `--category` with `completions` or `man` is an error, like with `why`.

### Changed
- apt repo is only rebuilt for the latest stable release, one run at a time; prereleases go to npm's `next` tag and skip apt.
- `.sha256` files come from the tarball already on the release; npm packages are built from checksum-verified binaries; checkouts keep no credentials; `flake.lock` is committed; packaging tool versions are pinned.

## 0.6.0 - 2026-10-23

### Added
- `safesweep completions <shell>` and a man page (`safesweep man`); the `.deb`, `.rpm` and release tarballs include them.
- Install from the apt repository, Homebrew tap, PyPI, npm/pnpm/yarn/bun, cargo-binstall, Nix, `.deb` and `.rpm`.

### Changed
- Releases publish to crates.io from CI. Re-running release workflows never replaces published assets.

## 0.5.1 - 2026-10-23

### Added
- Packaging only: cargo-binstall metadata, `.deb`/`.rpm`, Nix flake, PyPI wheels, npm packages.

## 0.5.0 - 2026-10-23

Safety fixes from a whole-repo review. Upgrade recommended.

### Fixed
- Files in use by processes in containers or other mount namespaces are detected (matched by inode), so they are no longer deleted.
- Running as root is refused when `HOME` is owned by another user (`sudo -E`) or a rule root is a symlink.
- Skip reasons and `why` output are escaped, so hostile file names or command lines cannot fake output or inject terminal escapes.
- A manifest write failure after a removal is reported instead of claiming nothing was deleted.
- `pycache` skips dot-directories, as documented; a relative `HOME` is rejected; a non-UTF-8 mount point no longer blocks every scan.
- TUI scans can be cancelled.

### Changed
- Runtime errors (unreadable `/proc`, terminal failure) exit 1 instead of 2. `--category` with `why` is an error.

## 0.4.2 - 2026-10-23

### Changed
- TUI: long paths are cut on the left, a Scanning screen shows during the first scan, and rows owned by other users are hidden.

## 0.4.1 - 2026-10-23

### Fixed
- File names with newlines or control characters could fake rows in `scan`, the TUI and the manifest. Paths are escaped everywhere.
- The TUI reports skipped and failed removals and exits 1 after a failure; Ctrl/Alt-modified keys no longer trigger actions.

## 0.4.0 - 2026-10-23

### Added
- `safesweep tui`: browse candidates, select the safe ones and delete them through the same guarded path as `clean --apply`.

## 0.3.2 - 2026-10-23

### Fixed
- A `~/.cache` symlinked into the temp dir no longer doubles the scan total.

### Added
- aarch64 Linux release binary.

## 0.3.1 - 2026-10-23

### Fixed
- `artifacts` rule: a hand-made `.venv` or `target` directory (for example `.venv/src/<pkg>` editable checkouts) is no longer taken for build output; trees containing `.git` are refused; the whole project must be unused; git activity includes other branches and linked worktrees; the walk no longer stops at the first unreadable directory.
- `why`: guest ticks counted twice, reused process IDs, zero-length samples, closed pipes, non-UTF-8 process names.

## 0.3.0 - 2026-10-23

### Added
- Opt-in `artifacts` rule: `node_modules`, `target` and `.venv` of git projects idle for 30 days.

## 0.2.1 - 2026-10-23

### Fixed
- JSON output escapes invalid UTF-8 in paths; clearer message when a removal fails partway.

## 0.2.0 - 2026-10-23

### Added
- `safesweep why`: CPU use per program and process.

## 0.1.0 - 2026-10-23

First release: `scan` and `clean [--apply]` with a fixed rule table, a guard that refuses anything open, recent, shared or outside its root, and a WSL compaction hint.
