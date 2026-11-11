# AGENTS.md

Instructions for coding agents (Claude Code, Codex, and others) working in this repository.
`CLAUDE.md` only imports this file, so this is the one place to edit.

## What this is

`safesweep` is a Rust CLI (Linux and WSL) that deletes caches and temp files only when it can prove they
are safe: nothing open, nothing recent, nothing shared. One crate is both the library (`src/lib.rs`) and
the command line (`src/main.rs`). Read [ARCHITECTURE.md](ARCHITECTURE.md) before changing behavior.

## Setup

Rust 1.88+ with `clippy` and `rustfmt`, on Linux or WSL. Some tests need unprivileged user namespaces
(`unshare -rm`); on Ubuntu 24.04+ run `sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0`,
otherwise they skip locally. Details: [CONTRIBUTING.md](CONTRIBUTING.md#set-up).

Git hooks: `lefthook install`. Pre-commit runs `gitleaks` on staged changes, `cargo fmt --check` and
`typos`; pre-push runs clippy.

## Checks (run before calling work done)

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings      # also fails on undocumented public items
RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps --document-private-items
cargo test --locked
```

Touching `npm/`, `install.sh` or shell scripts? Also run `node --test npm/test.js`, `sh tests/install_test.sh`
and the `shellcheck` lines in [CONTRIBUTING.md](CONTRIBUTING.md#everyday-commands). CI
(`.github/workflows/ci.yml`) runs all of these plus coverage (90% line minimum), link checking and package
builds.

## Rules that matter most

- `scan` observes, `guard` decides, `clean` deletes. New safety checks go in `guard::check` (or
  `scan::measure`), never in the interface.
- A change to what can be deleted needs a test that fails without it and shows what must be *refused*.
- Escape untrusted text (paths, process names, errors) with `render::escape_path` / `render::escape_text`.
- No panics on environmental input: unreadable or vanishing entries become skip reasons.
- Tests use `tempfile` directories and fake `Ctx` values; never real files. Never run
  `safesweep clean --apply` on this machine outside test fixtures.
- Adding a rule: see [CONTRIBUTING.md](CONTRIBUTING.md#adding-a-rule) for every place that must change.

## Commits and releases

Conventional Commits (`type(scope): summary`), small and atomic, reason in the body. Work on a branch off
`master`. Releases follow [RELEASING.md](RELEASING.md); `CHANGELOG.md` gets an entry for each.

## Environment variables

None are required. `CI` (set by GitHub Actions) turns namespace-test skips into failures.

## Agent workflow

Features are built spec-first with the [superpowers](https://github.com/obra/superpowers) skills:
brainstorm, write a design spec, write a step-by-step plan, then execute it task by task with subagents and
review. Completed specs and plans live in [`docs/agents/`](docs/agents/):

- [`specs/`](docs/agents/specs/): design decisions, dated by when they were written.
- [`plans/`](docs/agents/plans/): the implementation plans that executed them.

Shared agent config:

- `.claude/settings.json`: Claude Code plugins for this repo (rust-analyzer and clangd LSPs,
  commit-commands, ponytail). Personal overrides go in `.claude/settings.local.json` (gitignored).
- `.codex/config.toml`: the same ponytail plugin for Codex.
