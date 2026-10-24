# safesweep

[![CI](https://github.com/dpologdvinity/safesweep/actions/workflows/ci.yml/badge.svg)](https://github.com/dpologdvinity/safesweep/actions/workflows/ci.yml)
[![docs.rs](https://img.shields.io/docsrs/safesweep)](https://docs.rs/safesweep)
[![dependency status](https://deps.rs/repo/github/dpologdvinity/safesweep/status.svg)](https://deps.rs/repo/github/dpologdvinity/safesweep)
[![last commit](https://img.shields.io/github/last-commit/dpologdvinity/safesweep)](https://github.com/dpologdvinity/safesweep/commits/master)

[![release](https://img.shields.io/github/v/release/dpologdvinity/safesweep)](https://github.com/dpologdvinity/safesweep/releases)
[![crates.io](https://img.shields.io/crates/v/safesweep)](https://crates.io/crates/safesweep)
[![PyPI](https://img.shields.io/pypi/v/safesweep)](https://pypi.org/project/safesweep/)
[![npm](https://img.shields.io/npm/v/safesweep)](https://www.npmjs.com/package/safesweep)
[![Homebrew tap](https://img.shields.io/badge/homebrew-dpologdvinity%2Ftap-orange)](#install)

[![downloads](https://img.shields.io/crates/d/safesweep)](https://crates.io/crates/safesweep)
[![MSRV](https://img.shields.io/crates/msrv/safesweep)](Cargo.toml)
[![platform](https://img.shields.io/badge/platform-linux%20%7C%20WSL-blue)](#wsl)
[![license](https://img.shields.io/crates/l/safesweep)](#license)

**Clean caches and temp files that are provably safe to delete, and see what is eating your CPU.**

Most cleaners guess which files are junk. `safesweep` does not guess: it only looks at locations it has a
rule for (package-manager caches, `~/.cache`, temp files, ...), and before it deletes anything it checks
that nothing is using the files, that nothing in them changed recently, and that they are yours. If it
cannot prove a file is safe to remove, it leaves it alone and tells you why. On WSL it also explains how
to hand the freed disk space back to Windows.

Linux and WSL only (it reads `/proc` to see what programs are using).

![safesweep scan, clean and why](demo/demo.gif)

The animation runs against a throwaway home directory with a faked clock; `demo/make.sh` rebuilds it.

## Contents

[Quick start](#quick-start) · [Install](#install) · [Commands](#commands) ·
[How it decides what is safe](#how-it-decides-what-is-safe) · [Rules](#rules) · [WSL](#wsl) ·
[Exit codes](#exit-codes) · [FAQ](#faq) · [Development](#development) · [Documentation](#documentation) ·
[License](#license)

## Quick start

```sh
safesweep                  # 1. look: what could be removed, and why each item is or is not safe
safesweep clean            # 2. rehearse: what a cleanup would remove (deletes nothing)
safesweep clean --apply    # 3. do it
```

`safesweep` with no arguments is a read-only scan. Nothing is ever deleted without `--apply` (or the
confirmation prompt in `safesweep tui`).

## Install

Pick whichever fits your system:

```sh
curl -fsSL https://github.com/dpologdvinity/safesweep/releases/latest/download/install.sh | sh   # any Linux or WSL
cargo install safesweep                     # build from source (Rust 1.88 or newer)
cargo binstall safesweep                    # prebuilt binary, via cargo-binstall
brew install dpologdvinity/tap/safesweep    # Homebrew on Linux
pipx install safesweep                      # or: uv tool install safesweep
npm install -g safesweep                    # or: pnpm add -g / yarn global add / bun add -g
nix run github:dpologdvinity/safesweep      # Nix flake
```

**The `curl` installer** needs no root and no package manager. It downloads the release binary for your CPU
(x86_64 or aarch64), checks it against the published SHA-256 checksum, installs it into `~/.local/bin` (plus the man
page and shell completions under `~/.local/share`), and checks that the installed program reports the right
version. Read it first if you like: it is one short, commented file,
[`install.sh`](install.sh). It works with `curl` or `wget`.

```sh
# a specific version, into another directory:
curl -fsSL https://github.com/dpologdvinity/safesweep/releases/latest/download/install.sh | sh -s -- --version 0.6.5 --bin-dir /usr/local/bin
```

To uninstall, delete `safesweep` from the directory it installed into (the installer prints every path), along
with `~/.local/share/man/man1/safesweep.1` and the completion files it lists. If `~/.local/bin` is not in your
`PATH`, the installer tells you what to add.

**Debian, Ubuntu and WSL (apt repository, gets updates with `apt upgrade`):**

```sh
sudo install -d -m 0755 /etc/apt/keyrings
curl -fsSL https://dpologdvinity.github.io/safesweep/key.asc | sudo tee /etc/apt/keyrings/safesweep.asc >/dev/null
echo "deb [signed-by=/etc/apt/keyrings/safesweep.asc] https://dpologdvinity.github.io/safesweep stable main" | sudo tee /etc/apt/sources.list.d/safesweep.list
sudo apt update && sudo apt install safesweep
```

**Single files.** The [releases page](https://github.com/dpologdvinity/safesweep/releases) has `.deb` and
`.rpm` packages and static binaries for x86_64 and aarch64, each with a `.sha256` checksum:

```sh
sudo apt install ./safesweep_*_amd64.deb
sudo dnf install ./safesweep-*.x86_64.rpm
```

The `.deb`, `.rpm` and release tarballs include a man page and bash, zsh and fish completions. After
`cargo install`, set up completions yourself, for example
`safesweep completions bash > ~/.local/share/bash-completion/completions/safesweep`.

## Commands

| Command | What it does |
|---|---|
| `safesweep` or `safesweep scan` | Lists every candidate with its size, what it costs to get back, and `ok` or the reason it is skipped. Read-only. |
| `safesweep scan --json` | The same, as JSON, for scripts. |
| `safesweep clean` | Dry run: shows what would be removed and how much space it would free. Deletes nothing. |
| `safesweep clean --apply` | Removes what is `ok`, re-checking each item first, and logs every removal. |
| `safesweep tui` | Interactive list: pick the safe items and delete them after a confirmation. |
| `safesweep why` | Shows what is using the CPU right now. |
| `safesweep completions SHELL` | Prints a completion script (`bash`, `zsh`, `fish`, `elvish`, `powershell`). |

Option for `scan`, `clean` and `tui`: `--category ID` (repeatable) limits the run to some
[rules](#rules), for example `safesweep --category pip clean --apply`. Options for `why`: `--interval-ms N`
(how long to sample, at least 100, default 1000) and `--top N` (how many entries to list, default 10).

### The removal log

Every removal is written to `~/.local/state/safesweep/<unix-time>.log`, one line per path: the bytes
freed, a tab, the path. The log is created *before* anything is deleted: if it cannot be created, nothing
is removed. If it stops being writable partway through (a full disk, say), `safesweep` reports the removal
it just made and stops.

### `tui`: browse and delete interactively

`safesweep tui` shows the same candidates as `scan`, biggest first, with a size bar. Safe rows can be
selected; skipped rows are greyed out with their reason and cannot be selected. Rows owned by other users
are hidden (they can never be deleted).

| Key | Action |
|---|---|
| `j` / `k` or arrow keys | move |
| `space` | select or unselect the row |
| `a` / `n` | select all safe rows / clear the selection |
| `d` or `enter` | delete the selection (asks you to press `y`) |
| `q` or `esc` | quit (also cancels a running scan) |

Deleting uses exactly the same checks as `clean --apply`, and the list is rescanned afterwards.

### `why`: what is eating the CPU

`safesweep why` samples `/proc` twice and prints how busy the machine is, CPU per program (summed across
its processes, so twelve `rustc` jobs show as one line), and the busiest processes with their command
lines. Percentages are per core, like `top`, so a busy process on a 12-core machine can reach 1200%.
On WSL, only Linux processes are visible: if Task Manager shows `vmmem` busy but `why` is quiet, the load
is on the Windows side.

## How it decides what is safe

A candidate is **skipped**, with the reason shown, if any of these is true. The checks run when
scanning and again right before each deletion.

- It is a symlink, or lies outside its rule's directory.
- It is the temp directory, or contains it.
- Anything inside belongs to another user (unless run as root).
- Anything inside changed recently, within the rule's age (the newer of modification time and inode
  change time, so a tree unpacked with `tar` or `cp -p` today counts as new).
- It holds a socket or a named pipe (a live `tmux` or X11 socket does not show up as an open file).
- Anything inside is open, memory-mapped, being executed, or the working directory of one of your
  processes, including processes in containers or other mount namespaces (matched by inode, not path).
  Other users' processes are only visible when run as root.
- It contains a `.git` directory (an editable install or git dependency can hold uncommitted work).
- Any part of it cannot be read, it contains a read-only directory (deleting would stop halfway), or it is
  or contains a mount point.
- `/proc` cannot be read, so open files cannot be checked.
- You are root and it is in the temp directory (containers and services with a private `/tmp` cannot be
  checked from outside).

`HOME` must be an absolute path (and, as root, owned by root: use `sudo -H`, not `sudo -E`). `TMPDIR`
must be an absolute path that is not `/` and does not contain your home directory. Otherwise `safesweep`
refuses to run.

Sizes count only files with a single hard link: uv hardlinks its cache into virtual environments, so
deleting those files would free nothing, and `safesweep` does not pretend it would.

One honest limit: there is a tiny gap between the last check and the removal in which a program could open
a file. The check runs just before the removal, and on Linux a program that already has a file open keeps
working after it is deleted. The design and these trade-offs are explained in [ARCHITECTURE.md](ARCHITECTURE.md).

## Rules

A *rule* names a location and how to pick candidates in it. `safesweep` never considers a path no rule covers.

| Category | Location | Minimum age | Cost to get back |
|---|---|---|---|
| `xdg-cache` | each entry in `~/.cache` except `pip` and `uv` | 7 days | network |
| `pip` | `~/.cache/pip` | 1 day | network |
| `uv` | `~/.cache/uv` | 1 day | network |
| `npm` | `~/.npm/_cacache` | 1 day | network |
| `cargo` | `~/.cargo/registry/cache` | 1 day | network |
| `tmp` | each entry in `$TMPDIR` (or `/tmp`) | 7 days | free |
| `pycache` | every `__pycache__` under `~` (**opt-in**: `--category pycache`) | none | free |
| `artifacts` | `node_modules`, `target`, `.venv` of idle git projects (**opt-in**: `--category artifacts`) | 30 days | slow |

`xdg-cache` treats any entry of `~/.cache` untouched for 7 days as disposable, as the XDG standard
describes the directory. If you keep something valuable there (a large downloaded model, say), either move
it or check `safesweep scan` first: the table shows every candidate.

`pycache` and `artifacts` walk your home directory (skipping hidden directories), so they only run when you
name them. Both rules stay on the filesystem hosting your home directory, so projects on separate mounted
partitions or drives are deliberately not traversed.

`artifacts` is deliberately strict. A build directory is listed only if a project file sits next to it
(`package.json` for `node_modules`, `Cargo.toml` for `target`, `pyproject.toml`, `requirements.txt` or
`setup.py` for `.venv`), a file only the tool creates sits inside it (`pyvenv.cfg`, `CACHEDIR.TAG`,
`.package-lock.json`, ...), and the project's git history has been idle for 30 days (any commit, checkout or
fetch, on any branch or linked worktree, counts as activity). Projects without a `.git` directory, and
monorepo members without their own, are never listed. The whole project must also be unused by any of your
processes, so a long-running server keeps its dependencies.

## WSL

On WSL 2 the Linux filesystem lives in one virtual disk file, `ext4.vhdx`. Deleting files inside Linux
frees space inside that file but does not make the file smaller, so Windows' free space does not change.
After `clean --apply` frees space on WSL, `safesweep` prints the steps to compact the disk: export a backup,
`wsl --shutdown`, then `Optimize-VHD` (or `diskpart`). It only prints them; it never runs them.

It deliberately does not suggest `wsl --manage --set-sparse`: WSL disabled sparse disks after data-corruption
reports, and a sparse disk cannot be compacted with `Optimize-VHD`.

## Exit codes

| Code | Meaning |
|---|---|
| `0` | success |
| `1` | the command ran but something failed: a removal failed or could not be logged, `/proc` or the terminal was unusable |
| `2` | bad usage: unknown `--category`, unsafe `HOME` or `TMPDIR`, a flag that does not apply to the command |

## FAQ

**Is it safe to run?** `scan` and `clean` (without `--apply`) change nothing. `clean --apply` deletes only
what passed every check [listed above](#how-it-decides-what-is-safe), and only caches and temp files the rules
name, which programs recreate on demand.

**Why was something skipped?** The status column says why: `modified within 7 days`, `in use by a process`,
`owned by another user`, and so on. Skipping is the tool being careful, not an error.

**Can I undo a cleanup?** No: deleted files are gone. That is why it only touches things that can be
recreated. The log in `~/.local/state/safesweep/` lists exactly what was removed.

**Why did the disk not get smaller on WSL?** See [WSL](#wsl).

**Does it work on macOS or Windows?** No. It relies on `/proc`. On Windows, use it inside WSL.

**`safesweep --category pycache` is slow.** It walks your whole home directory. Seconds on a normal
home, longer on very large ones.

**Why not `sudo -E`?** It keeps your `HOME`, which would let a user-controlled directory steer a
root-level delete, so `safesweep` refuses it. Run it as yourself: it does not need root.

## Development

You need Rust 1.88 or newer. Node 22 is only needed for the npm launcher tests.

```sh
cargo build                                   # build
cargo test                                    # all tests (a few use `unshare -rm`, see CONTRIBUTING.md)
cargo clippy --all-targets -- -D warnings     # lint; also fails on undocumented public items
cargo fmt                                     # format
cargo doc --no-deps --open                    # browse the API documentation
cargo llvm-cov --summary-only                 # line coverage (needs cargo-llvm-cov)
node --test npm/test.js                       # npm launcher tests
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for the workflow, conventions and how to add a rule.

## Documentation

- [ARCHITECTURE.md](ARCHITECTURE.md): how it is built and why; the safety model.
- [CONTRIBUTING.md](CONTRIBUTING.md): set up, test, and submit a change.
- [SECURITY.md](SECURITY.md): how to report a vulnerability.
- [RELEASING.md](RELEASING.md): how a release is made and published.
- [CHANGELOG.md](CHANGELOG.md): what changed in each version.
- API reference: `cargo doc --open`, or [docs.rs/safesweep](https://docs.rs/safesweep).

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option.
