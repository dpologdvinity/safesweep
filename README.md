# safesweep

Clean caches and temp files that are provably safe to delete. Nothing open, nothing recent, nothing shared.

Most cleaners guess. safesweep only touches known cache locations, and before deleting anything it proves no running process is using it. On WSL, it also tells you how to give the freed space back to Windows.

Linux and WSL only.

## Install

```sh
cargo install safesweep
```

## Usage

```sh
safesweep                     # list candidates and why each is or isn't safe (same as `safesweep scan`)
safesweep scan --json         # same, as JSON
safesweep clean               # dry run: what would be removed
safesweep clean --apply       # remove it
safesweep --category pip clean --apply   # limit to one rule (repeatable)
```

Every removal is logged to `~/.local/state/safesweep/<unix-time>.log`. If that log cannot be written, nothing is deleted.

Exit codes: `0` success, `1` some removals failed, `2` bad usage.

## What makes a candidate safe

A candidate is skipped, with the reason shown, if any of these hold:

- it is a symlink, or resolves outside its rule's directory
- anything inside is owned by another user (unless run as root)
- anything inside changed within the rule's age threshold (newer of mtime and ctime, so a tree unpacked with `tar` or `cp -p` today counts as new)
- it contains a socket or FIFO (live tmux or X11 sockets don't show up as open files)
- anything inside is open, memory-mapped, executing, or the working directory of any process
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

`pycache` walks your whole home directory, which can take minutes, so it only runs when named.

## WSL

Deleting files inside WSL doesn't shrink the `ext4.vhdx` disk image on Windows. After `clean --apply` frees space on WSL, safesweep prints the steps to compact it: export a backup, `wsl --shutdown`, then `Optimize-VHD` or `diskpart`. It never runs them for you.

It does not recommend `wsl --manage --set-sparse`. WSL disabled sparse VHDs over data-corruption reports, and a sparse disk can't be compacted with `Optimize-VHD`.

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option.
