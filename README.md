# safesweep

Clean caches and temp files that are provably safe to delete, and see what is eating your CPU.

Most cleaners guess. safesweep only touches known cache locations, and before deleting anything it proves no running process is using it. On WSL, it also tells you how to give the freed space back to Windows.

Linux and WSL only.

![safesweep scan, clean and why](demo/demo.gif)

The demo runs against a throwaway home directory with a faked clock; `demo/make.sh` rebuilds it.

## Install

```sh
cargo install safesweep
```

Or download a prebuilt static Linux binary (x86_64 or aarch64) from the [releases page](https://github.com/dpologdvinity/safesweep/releases) (each tarball has a `.sha256` file next to it).

## Usage

```sh
safesweep                     # list candidates and why each is or isn't safe (same as `safesweep scan`)
safesweep scan --json         # same, as JSON
safesweep clean               # dry run: what would be removed
safesweep clean --apply       # remove it
safesweep --category pip clean --apply   # limit to one rule (repeatable)
safesweep why                 # what is using the CPU right now
safesweep tui                 # browse, select and delete interactively
```

## `tui`: browse and delete interactively

`safesweep tui` shows the same candidates as `scan`, biggest first, with a size bar. Safe rows can be selected; skipped rows are greyed out with the reason and cannot be selected.

| Key | Action |
|---|---|
| `j` / `k` or arrows | move |
| `space` | select or unselect the row |
| `a` / `n` | select all safe rows / none |
| `d` or `enter` | delete the selection (asks for `y` to confirm) |
| `q` or `esc` | quit |

Deleting runs the same guarded path as `clean --apply`: every candidate is re-checked right before removal and logged to the manifest. The list is rescanned afterwards. `--category` works here too.

## `why`: what is eating the CPU

`safesweep why` samples `/proc` for one second (`--interval-ms`, at least 100) and prints how busy the machine is, CPU per program (summed across its processes, so twelve `rustc` jobs show as one line), and the busiest processes with their command lines. Percentages are per core, like `top`.

On WSL, only Linux processes are visible. If Task Manager shows `vmmem` busy but `why` is quiet, the load is on the Windows side.

Every removal is logged to `~/.local/state/safesweep/<unix-time>.log`. If that log cannot be written, nothing is deleted.

Exit codes: `0` success, `1` some removals failed, `2` bad usage.

## What makes a candidate safe

A candidate is skipped, with the reason shown, if any of these hold:

- it is a symlink, or resolves outside its rule's directory
- anything inside is owned by another user (unless run as root)
- anything inside changed within the rule's age threshold (newer of mtime and ctime, so a tree unpacked with `tar` or `cp -p` today counts as new)
- it contains a socket or FIFO (live tmux or X11 sockets don't show up as open files)
- anything inside is open, memory-mapped, executing, or the working directory of any process
- it contains a `.git` directory (an editable install or git dependency can hold uncommitted work)
- any part of it can't be read, it contains a read-only directory (deletion would stop halfway), or it is or contains a mount point
- `/proc` can't be read, so open files can't be checked
- it is in the temp dir and safesweep runs as root (services with a private `/tmp` and containers can't be checked from outside)

`TMPDIR` must be an absolute path that is not `/` and does not contain your home directory; otherwise safesweep refuses to run.

These checks run when scanning and again right before each deletion.

Reported sizes count only files with a single hard link. uv hardlinks its cache into virtualenvs, so deleting those files would free nothing, and safesweep doesn't pretend it would.

## Rules

| Category | Location | Minimum age | Cost to regenerate |
|---|---|---|---|
| `xdg-cache` | each entry in `~/.cache` except `pip` and `uv` | 7 days | network |
| `pip` | `~/.cache/pip` | 1 day | network |
| `uv` | `~/.cache/uv` | 1 day | network |
| `npm` | `~/.npm/_cacache` | 1 day | network |
| `cargo` | `~/.cargo/registry/cache` | 1 day | network |
| `tmp` | each entry in `$TMPDIR` (or `/tmp`) | 7 days | free |
| `pycache` | every `__pycache__` under `~` (opt-in: `--category pycache`) | none | free |
| `artifacts` | `node_modules`, `target`, `.venv` in idle git projects (opt-in: `--category artifacts`) | 30 days | slow |

`pycache` and `artifacts` walk your home directory (skipping dot-directories), so they only run when named.

`artifacts` only lists a build directory when its project has a marker file next to it (`package.json` for `node_modules`, `Cargo.toml` for `target`, `pyproject.toml`, `requirements.txt` or `setup.py` for `.venv`) and its git history is idle: nothing touched `.git/logs/HEAD` or `.git/index` for 30 days. Git activity includes other branches and linked worktrees. The build directory must also contain a file only the tool creates (`pyvenv.cfg`, `CACHEDIR.TAG`, `.package-lock.json`, ...), so a hand-made folder named `target` is never touched. The project directory must be unused by any process, not just the build directory, so a long-running service keeps its dependencies. Projects without `.git` and monorepo members without their own `.git` are never listed. Every normal safety check still applies, so a build directory used in the last 30 days is skipped even if nothing was committed.

## WSL

Deleting files inside WSL doesn't shrink the `ext4.vhdx` disk image on Windows. After `clean --apply` frees space on WSL, safesweep prints the steps to compact it: export a backup, `wsl --shutdown`, then `Optimize-VHD` or `diskpart`. It never runs them for you.

It does not recommend `wsl --manage --set-sparse`. WSL disabled sparse VHDs over data-corruption reports, and a sparse disk can't be compacted with `Optimize-VHD`.

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option.
