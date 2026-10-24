# Contributing

Thanks for helping. `safesweep` deletes files, so changes are held to a high bar: a change to what can be
deleted needs a test that fails without it, and a clear reason in the description. Everything else is
ordinary.

Please read [ARCHITECTURE.md](ARCHITECTURE.md) first (it is short and explains where things live and why),
and follow the [security policy](SECURITY.md) for vulnerabilities rather than opening a public issue.

## Set up

You need:

- **Rust 1.88 or newer** (`rustup` is the easiest way) with `clippy` and `rustfmt`.
- **Linux or WSL.** The program and its tests read `/proc` and use Unix file semantics.
- `unshare` (from util-linux) for a few tests. On Ubuntu 24.04 and later, unprivileged user namespaces
  are blocked by default; to run those tests locally:
  `sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0`. Without this the tests skip with a
  message locally (and **fail** on CI, on purpose; see below).
- Optional: Node 22 (npm launcher tests), `shellcheck`, Docker (to run `actionlint` as CI does),
  `cargo-llvm-cov` (coverage).

```sh
git clone https://github.com/dpologdvinity/safesweep
cd safesweep
cargo build
```

## Everyday commands

```sh
cargo test                                    # unit and end-to-end tests
cargo clippy --all-targets -- -D warnings     # lint (also fails on any undocumented public item)
cargo fmt                                     # format; CI runs `cargo fmt --check`
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items   # docs must build, links must resolve
cargo llvm-cov --summary-only                 # coverage; CI requires at least 90% of lines
node --test npm/test.js                       # npm launcher tests
shellcheck -s bash apt/build.sh npm/build.sh npm/publish.sh packaging/gen-extras.sh demo/make.sh demo/env.sh
```

CI runs all of these (`.github/workflows/ci.yml`) on every pull request, in stages that stop early on a
failure: setup, lint, test (newest Rust and Rust 1.88), coverage, then a build of every package format.
A pull request is mergeable when all jobs are green.

## Making a change

1. **Branch** from `master`: `git switch -c fix/short-description`.
2. **Write the test first** for any behavior change or bug fix, and watch it fail for the right reason.
   For something that deletes, the test should show the thing it must *refuse* as well as what it allows.
3. **Make the change**, small and focused. Keep the three roles apart (see ARCHITECTURE.md): `scan`
   observes, `guard` decides, `clean` deletes. A new safety rule belongs in `guard::check` (or, if it is
   about what a tree contains, in `scan::measure`), never in the interface.
4. **Document it.** Every public item needs a doc comment (the build fails otherwise). Explain *why*,
   mention errors and edge cases, and keep comments true: update them when the code changes.
5. **Run the checks** from the list above.
6. **Commit** with a [Conventional Commit](https://www.conventionalcommits.org/) message:
   `type(scope): summary` with a type of `feat`, `fix`, `refactor`, `perf`, `test`, `docs`, `build`,
   `ci` or `chore`. Keep commits small: one logical change each, with the reason in the body.
7. **Open a pull request** that says what changed, why, and how you checked it. Link the issue if any.

## Conventions

- **Safety first.** A new reason to skip a candidate is cheap to add and cheap to defend; a new way to be
  *more* permissive needs strong justification and a test.
- **Untrusted text.** File names, error messages, process names and command lines can contain anything.
  Pass them through `render::escape_path` / `render::escape_text` before printing, logging or serializing.
- **No panics on environmental input.** Unreadable, vanishing or malformed entries become a skip reason,
  not an `unwrap`.
- **Tests do not touch real files.** Use temporary directories (`tempfile`) and fake `Ctx` values. A file's
  inode change time cannot be back-dated, so tests move the clock (`Ctx::now`) to make files "old".
- **Namespace tests must really run on CI.** Tests that need `unshare -rm` call `namespaces_usable()`,
  which skips on a developer machine but panics when the `CI` environment variable is set, so a safety
  regression test cannot silently stop running.
- **Speed claims need measurements.** If a change is about performance, include before and after numbers.

## Adding a rule

A rule is one entry in `RULES` in `src/rules.rs`:

```rust
Rule {
    id: "gradle",                       // shown in output; accepted by --category
    root: Root::Home(".gradle/caches"), // relative to $HOME (or Root::Tmp)
    matcher: Matcher::Whole,            // the directory itself; or Children, NamedDirs, ...
    min_age_days: 7,                    // nothing inside may have changed for this long
    regen: Regen::Network,              // what it costs to get the data back
    opt_in: false,                      // true if slow or heavy: only runs when named
},
```

Then update the expected lists in the tests at the bottom of `rules.rs`, the rules table in the README
and ARCHITECTURE.md, and the changelog. If an existing `Matcher` does not fit, add one in `src/scan.rs` with
tests; the guard and the cleaner need no changes, and every safety check applies automatically.

## Releases

Maintainers cut releases; see [RELEASING.md](RELEASING.md). The version is the one in `Cargo.toml`, and
`CHANGELOG.md` gets an entry for every release.

## Code of conduct

Be kind, assume good faith, and keep discussion about the code. Harassment of any kind is not welcome.
