# safesweep stage 2: project build artifacts

Extends the stage 1 spec. One new opt-in rule, `artifacts`, run with `--category artifacts`.

## Behavior

Finds regenerable build directories in projects nobody has touched for a while:

| Directory | Counts as an artifact only if a sibling exists |
|---|---|
| `node_modules` | `package.json` |
| `target` | `Cargo.toml` |
| `.venv` | `pyproject.toml`, `requirements.txt`, or `setup.py` |

The sibling check stops it from deleting an unrelated folder that happens to be called `target`.

A project is **idle** when its last git activity is more than 30 days ago. Git activity is the newer mtime of `.git/logs/HEAD` and `.git/index` (no subprocess, no git dependency). A project without `.git` is never listed, because there is no reliable activity signal.

An idle project's artifact is then still subject to every stage 1 guard (tree changed within 30 days, open files, mounts, ownership, ...). So an artifact someone built into yesterday is skipped even if nothing was committed.

## Rule

`id: "artifacts"`, root `$HOME`, `min_age_days: 30`, `regen: Slow`, `opt_in: true`. Opt-in because it walks the home directory and deletes things that cost minutes to rebuild.

## Walk

Depth limit 6 from `$HOME`. Does not follow symlinks or cross filesystems. Does not enter dot-directories (`.cache`, `.rustup`, `.npm`, ...), does not enter `node_modules`, and does not descend into a directory it matched.

## Out of scope

Other build dirs (`dist`, `build`, `.next`), non-git projects, configurable idle time, a `--path` option.
